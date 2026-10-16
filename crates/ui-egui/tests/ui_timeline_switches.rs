//! Timeline layer switches with real pointer input (egui_kittest): pressing a switch and
//! dragging over other layers gives them all the state the first one got, in one undo step, as
//! in After Effects (#227); stopwatches the same way (#284). Also the label swatch's menu and the
//! layer bar past its source (#290).

use effectcraft_engine::Session;
use effectcraft_engine::color::Label;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Color32, Event, Modifiers, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

/// Comp "Main" with solids A (top) to D (bottom); returns their ids top to bottom.
fn harness() -> (Harness<'static, EffectcraftApp>, Vec<u64>) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let mut ids: Vec<u64> = ["D", "C", "B", "A"]
        .iter()
        .map(|name| s.execute("layer.newSolid", json!({"name": name, "color": "#406080"})).unwrap()["layer"].as_u64().unwrap())
        .collect();
    ids.reverse();
    s.execute("edit.deselectAll", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    (h, ids)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

/// Press on `from`, move to `to` in `steps`, release there.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, steps: u32) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=steps {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / steps as f32)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// Each layer's switch, top to bottom.
fn states(h: &Harness<'_, EffectcraftApp>, ids: &[u64], f: fn(&effectcraft_engine::project::Layer) -> bool) -> Vec<bool> {
    let comp = h.state().session.active_comp().unwrap();
    ids.iter().map(|id| f(comp.layer(LayerId(*id)).unwrap())).collect()
}

fn video(l: &effectcraft_engine::project::Layer) -> bool {
    l.switches.video
}

fn motion_blur(l: &effectcraft_engine::project::Layer) -> bool {
    l.switches.motion_blur
}

/// A shape's eye hides its pixels without changing Opacity or animation, and Undo/Redo restores
/// that visibility. Cameras, lights and nulls use the same switch even though they have no pixels.
#[test]
fn layer_eyes_hide_shape_pixels_and_enable_camera_light_and_null() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 36, "duration": 1})).unwrap();
    let shape = s.execute("layer.newShape", json!({"kind": "rect", "size": [24, 16]})).unwrap()["layer"].as_u64().unwrap();
    for (time, value) in [(0.0, 100), (1.0, 40)] {
        s.execute("prop.addKey", json!({"layer": shape, "path": "transform/opacity", "time": time, "value": value})).unwrap();
    }
    let mut others = Vec::new();
    for command in ["layer.newNull", "layer.newLight", "layer.newCamera"] {
        others.push(s.execute(command, json!({})).unwrap()["layer"].as_u64().unwrap());
    }
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let alpha = |h: &Harness<'_, EffectcraftApp>| {
        let s = &h.state().session;
        effectcraft_engine::render::Renderer::new(&s.project, &effectcraft_engine::render::NoFootage, Default::default())
            .comp_frame(s.active_comp_id().unwrap(), s.time())
            .get(32, 18)[3]
    };
    let opacity = |h: &Harness<'_, EffectcraftApp>| {
        h.state().session.active_comp().unwrap().layer(LayerId(shape)).unwrap().props.prop("transform/opacity").unwrap().clone()
    };
    let initial_opacity = opacity(&h);
    let before = h.state().session.history.undo.len();
    let eye_id = format!("timeline.layer.{shape}.video");
    assert_eq!(h.state().auto.find(&eye_id).unwrap().label, "Hide Layer");
    assert!(alpha(&h) > 0.99);
    let at = rect(&h, &eye_id).center();
    drag(&mut h, at, at, 1);
    assert_eq!(states(&h, &[shape], video), [false]);
    assert!(alpha(&h) < 0.01, "the hidden shape has no rendered pixels");
    assert_eq!(opacity(&h), initial_opacity, "visibility leaves opacity intact");
    assert_eq!(h.state().auto.find(&eye_id).unwrap().label, "Show Layer");
    assert_eq!(h.state().session.history.undo.len(), before + 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert!(alpha(&h) > 0.99);
    h.state_mut().session.execute("edit.redo", json!({})).unwrap();
    h.run_steps(2);
    assert!(alpha(&h) < 0.01);
    for id in others {
        let at = rect(&h, &format!("timeline.layer.{id}.video")).center();
        drag(&mut h, at, at, 1);
        assert_eq!(states(&h, &[id], video), [false]);
    }
    assert!(h.state().session.active_comp().unwrap().active_camera(h.state().session.time()).is_none(), "the camera's eye disables it");
}

/// A/V Features is an optional column: hiding it must not strand a hidden layer. The row's
/// context menu can hide/show it and remains an undoable engine command.
#[test]
fn hidden_av_column_keeps_layer_visibility_in_the_context_menu() {
    let (mut h, ids) = harness();
    let layer = ids[0];
    h.state_mut().ui.timeline.columns.remove("av");
    h.run_steps(2);
    assert!(h.state().auto.find(&format!("timeline.layer.{layer}.video")).is_none());
    for (label, expected) in [("Hide Layer", false), ("Show Layer", true)] {
        let row = rect(&h, &format!("timeline.layer.{layer}.row"));
        let at = pos2(row.min.x + 140.0, row.center().y);
        h.event(Event::PointerMoved(at));
        h.step();
        h.event(Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        h.event(Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(3);
        let id = format!("timeline.layer.{layer}.context.visibility");
        assert_eq!(h.state().auto.find(&id).unwrap().label, label);
        let button = rect(&h, &id).center();
        let before = h.state().session.history.undo.len();
        drag(&mut h, button, button, 1);
        assert_eq!(states(&h, &[layer], video), [expected]);
        assert_eq!(h.state().session.history.undo.len(), before + 1);
        assert!(h.state().auto.find(&id).is_none(), "the menu closes after toggling");
    }
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(states(&h, &[layer], video), [false]);
}

#[test]
fn dragging_over_layer_switches_sets_them_all() {
    let (mut h, ids) = harness();
    let eye = |h: &Harness<'_, EffectcraftApp>, i: usize| rect(h, &format!("timeline.layer.{}.video", ids[i])).center();
    let steps = h.state().session.history.undo.len();

    // From A's eye down to D's: all hidden, one undo step.
    let (a, d) = (eye(&h, 0), eye(&h, 3));
    drag(&mut h, a, d, 10);
    assert_eq!(states(&h, &ids, video), [false; 4]);
    assert_eq!(h.state().session.history.undo.len(), steps + 1, "one undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(states(&h, &ids, video), [true; 4]);

    // Every layer gets the first one's new state, whatever its own: B hidden, then from C (shown
    // → hidden) up to A, quickly (two moves): B stays hidden, A is hidden, D untouched.
    h.state_mut().session.execute("layer.setSwitch", json!({"layers": [ids[1]], "switch": "video", "value": false})).unwrap();
    h.run_steps(2);
    let (c, a) = (eye(&h, 2), eye(&h, 0));
    drag(&mut h, c, a, 2);
    assert_eq!(states(&h, &ids, video), [false, false, false, true]);
    // From B (hidden → shown) down to D: B, C and D shown.
    let (b, d) = (eye(&h, 1), eye(&h, 3));
    drag(&mut h, b, d + vec2(0.0, 40.0), 6);
    assert_eq!(states(&h, &ids, video), [false, true, true, true]);

    // A click still toggles only that layer.
    let c = eye(&h, 2);
    drag(&mut h, c, c, 1);
    assert_eq!(states(&h, &ids, video), [false, true, false, true]);

    // The Switches column works the same (Motion Blur, from D up to B).
    let mb = |h: &Harness<'_, EffectcraftApp>, i: usize| rect(h, &format!("timeline.layer.{}.switch.motionBlur", ids[i])).center();
    let (d, b) = (mb(&h, 3), mb(&h, 1));
    drag(&mut h, d, b, 8);
    assert_eq!(states(&h, &ids, motion_blur), [false, true, true, true]);
    assert_eq!(states(&h, &ids, video), [false, true, false, true], "other switches untouched");
}

/// Alt-click on an A/V feature sets the layer apart (the other layers hidden / locked), and the
/// next Alt-click brings the others back; the Switches column is not affected by Alt.
#[test]
fn alt_click_on_av_feature_switches_the_other_layers() {
    let (mut h, ids) = harness();
    let alt_click = |h: &mut Harness<'_, EffectcraftApp>, id: String| {
        let p = rect(h, &id).center();
        let modifiers = Modifiers::ALT;
        h.event(Event::PointerMoved(p));
        h.event(Event::ModifiersChanged(modifiers));
        h.step();
        h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers });
        h.step();
        h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers });
        h.event(Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(3);
    };
    let steps = h.state().session.history.undo.len();
    alt_click(&mut h, format!("timeline.layer.{}.video", ids[1]));
    assert_eq!(states(&h, &ids, video), [false, true, false, false]);
    assert_eq!(h.state().session.history.undo.len(), steps + 1, "one undo step");
    alt_click(&mut h, format!("timeline.layer.{}.video", ids[1]));
    assert_eq!(states(&h, &ids, video), [true; 4]);
    alt_click(&mut h, format!("timeline.layer.{}.lock", ids[2]));
    assert_eq!(states(&h, &ids, |l| l.switches.locked), [true, true, false, true]);
    alt_click(&mut h, format!("timeline.layer.{}.solo", ids[0]));
    assert_eq!(states(&h, &ids, |l| l.switches.solo), [true, false, false, false]);
    alt_click(&mut h, format!("timeline.layer.{}.switch.motionBlur", ids[0]));
    assert_eq!(states(&h, &ids, motion_blur), [true, false, false, false]);
}

/// #284: pressing a stopwatch and dragging over the stopwatches below gives every property the
/// state the first one got (animated or not), in one undo step; Alt+click still adds an
/// expression and a click toggles one property.
#[test]
fn dragging_over_stopwatches_sets_them_all() {
    let (mut h, ids) = harness();
    // Layer A twirled open to its Transform properties.
    let (layer, uids) = {
        let comp = h.state().session.active_comp().unwrap();
        let l = comp.layer(LayerId(ids[0])).unwrap();
        let tr = l.props.sub("transform").unwrap();
        let uids: Vec<u64> = ["anchor", "position", "scale", "rotation", "opacity"].iter().map(|m| tr.get(m).unwrap().uid).collect();
        (l.id, (tr.uid, uids))
    };
    let (group, uids) = uids;
    h.state_mut().ui.timeline.open_layers.insert(layer.0);
    h.state_mut().ui.timeline.open_groups.insert(group);
    h.run_steps(3);
    let animated = |h: &Harness<'_, EffectcraftApp>| -> Vec<bool> {
        let comp = h.state().session.active_comp().unwrap();
        let l = comp.layer(layer).unwrap();
        uids.iter().map(|u| l.props.find(*u).unwrap().is_animated()).collect()
    };
    let sw = |h: &Harness<'_, EffectcraftApp>, i: usize| rect(h, &format!("timeline.prop.{}.stopwatch", uids[i])).center();
    let steps = h.state().session.history.undo.len();

    // Scale animated first; from Anchor Point down to Opacity: all animated, one undo step.
    h.state_mut().session.execute("prop.toggleAnimation", json!({"layer": layer.0, "prop": uids[2]})).unwrap();
    h.run_steps(2);
    let steps = steps + 1;
    let (a, o) = (sw(&h, 0), sw(&h, 4));
    drag(&mut h, a, o, 10);
    assert_eq!(animated(&h), [true; 5]);
    assert_eq!(h.state().session.history.undo.len(), steps + 1, "one undo step");
    // From Rotation (animated → not) up to Position, quickly: those three stop animating.
    let (r, p) = (sw(&h, 3), sw(&h, 1));
    drag(&mut h, r, p, 2);
    assert_eq!(animated(&h), [true, false, false, false, true]);
    // A click toggles only that property.
    let s = sw(&h, 2);
    drag(&mut h, s, s, 1);
    assert_eq!(animated(&h), [true, false, true, false, true]);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(animated(&h), [true; 5], "each drag was one step");
}

/// A swatch (a small rect filled with `col`) painted this frame in the left half of `row`.
fn swatch_in(h: &Harness<'_, EffectcraftApp>, row: Rect, col: Color32) -> bool {
    fn walk(s: &egui::Shape, row: Rect, col: Color32) -> bool {
        match s {
            egui::Shape::Rect(r) => r.fill == col && r.rect.width() < 16.0 && row.contains(r.rect.center()) && r.rect.center().x < row.center().x,
            egui::Shape::Vec(v) => v.iter().any(|s| walk(s, row, col)),
            _ => false,
        }
    }
    h.output().shapes.iter().any(|c| walk(&c.shape, row, col))
}

/// Clicking a layer's label swatch opens the label menu, which shows each label's colour before
/// its name, so a label is picked by its colour (#290).
#[test]
fn the_label_swatch_menu_shows_the_label_colours() {
    let (mut h, ids) = harness();
    let green = h.state().session.prefs.label_name(Label::Green);
    let at = rect(&h, &format!("timeline.layer.{}.label", ids[1])).center();
    drag(&mut h, at, at, 1);
    let entry = h.query_by_label(&green).expect("the label menu is open").rect();
    for l in [Label::Green, Label::SeaFoam, Label::Purple] {
        let e = h.query_by_label(&h.state().session.prefs.label_name(l)).unwrap().rect();
        assert!(swatch_in(&h, e, h.state().tokens.label(l)), "no {l:?} swatch");
    }
    let undo = h.state().session.history.undo.len();
    drag(&mut h, entry.center(), entry.center(), 1);
    assert_eq!(h.state().session.active_comp().unwrap().layer(LayerId(ids[1])).unwrap().label, Label::Green);
    assert_eq!(h.state().session.history.undo.len(), undo + 1);
    assert!(h.query_by_label(&green).is_none(), "the menu closed after the choice");
}

/// A layer extended past its source's end (its last frame held) has that part of its bar
/// striped, so the source's own span shows (#290); with time remapping it isn't.
#[test]
fn the_bar_past_the_source_end_is_striped() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    s.execute("comp.new", json!({"name": "Outer", "width": 64, "height": 64, "frameRate": 30, "duration": 4})).unwrap();
    let l = s.execute("layer.addItem", json!({"item": "Inner"})).unwrap()["layer"].as_u64().unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let past = format!("timeline.layer.{l}.bar.afterSource");
    assert!(h.state().auto.find(&past).is_none(), "the source fills the whole bar");
    h.state_mut().session.execute("layer.timing", json!({"layers": [l], "out": 4.0})).unwrap();
    h.run_steps(2);
    let (bar, after) = (rect(&h, &format!("timeline.layer.{l}.bar")), rect(&h, &past));
    assert!((after.min.x - bar.center().x).abs() < 2.0 && (after.max.x - bar.max.x).abs() < 1.0, "{bar:?} {after:?}");
    h.state_mut().session.execute("layer.enableTimeRemap", json!({"layers": [l]})).unwrap();
    h.run_steps(2);
    assert!(h.state().auto.find(&past).is_none(), "time remapping decides what shows");
}
