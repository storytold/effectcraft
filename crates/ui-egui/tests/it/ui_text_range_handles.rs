//! #1011: a text layer's Range Selector shows its Start and End as bars on the text in the
//! Composition viewer, as in After Effects, while the selector or its animator is selected. A
//! drag sets Start / End in one undo step (a keyframe at the current time when animated) and
//! doesn't move the layer. Uses only APIs that existed before the bars, so it runs (and fails)
//! on the code without them.

use effectcraft_engine::Session;
use effectcraft_engine::commands::text_edit::layer_doc;
use effectcraft_engine::project::{LayerId, PropGroup};
use effectcraft_engine::text::layout_doc;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::panels::viewer::comp_to_screen;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

struct Ids {
    layer: u64,
    animator: u64,
    color: u64,
    start: u64,
    end: u64,
}

fn animator(s: &Session, l: u64) -> PropGroup {
    let layer = s.active_comp().unwrap().layer(LayerId(l)).unwrap();
    layer.props.sub("text").unwrap().sub("animators").unwrap().groups().next().unwrap().clone()
}

/// A 640×360 comp with the point text "abcdefgh" and a Fill Color animator (one Range Selector).
fn setup() -> (Session, Ids) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "duration": 4})).unwrap();
    let l = s.execute("layer.newText", json!({"text": "abcdefgh", "size": 60})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addTextAnimator", json!({"layer": l, "property": "fillColor"})).unwrap();
    let a = animator(&s, l);
    let sel = a.sub("selectors").unwrap().groups().next().unwrap();
    let color = a.sub("properties").unwrap().children.first().unwrap().uid();
    let ids = Ids { layer: l, animator: a.uid, color, start: sel.get("start").unwrap().uid, end: sel.get("end").unwrap().uid };
    s.state.selected_layers = vec![LayerId(l)];
    (s, ids)
}

fn harness(s: Session) -> Harness<'static, EffectcraftApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);
    h
}

fn handle(h: &Harness<'_, EffectcraftApp>, uid: u64) -> Option<Pos2> {
    h.state().auto.find(&format!("viewer.textRange.{uid}")).map(|e| pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0))
}

fn value(h: &Harness<'_, EffectcraftApp>, ids: &Ids, uid: u64) -> f64 {
    h.state().session.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().props.find(uid).unwrap().value.as_f64()
}

/// Screen x of the caret before character `ci` of the text.
fn caret_x(h: &Harness<'_, EffectcraftApp>, ids: &Ids, ci: usize) -> f32 {
    let s = &h.state().session;
    let lay = layout_doc(&layer_doc(s, LayerId(ids.layer)).unwrap());
    let pos = s.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().props.prop("transform/position").unwrap().value.as_vec2();
    let (_, bottom) = lay.caret(ci);
    comp_to_screen(&h.ctx, [(pos[0] + bottom.x) as f32, (pos[1] + bottom.y) as f32]).unwrap().x
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=8 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// With the Range Selector's animator selected, End is a bar at the end of the text; dragging it
/// back to the boundary after the fourth character sets End to 50%, in one undo step, without
/// moving Start or the layer.
#[test]
fn dragging_end_sets_it_in_one_undo_step() {
    let (mut s, ids) = setup();
    s.state.selected_props = vec![(LayerId(ids.layer), ids.animator)];
    let mut h = harness(s);
    let at = handle(&h, ids.end).unwrap_or_else(|| panic!("no viewer.textRange.{}: the Range Selector's End isn't in the viewer", ids.end));
    assert!(handle(&h, ids.start).is_some(), "and its Start");
    let position = h.state().session.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().props.prop("transform/position").unwrap().value.clone();
    let undo = h.state().session.history.undo.len();
    let dx = caret_x(&h, &ids, 4) - caret_x(&h, &ids, 8);
    drag(&mut h, at, at + vec2(dx, 0.0));
    let end = value(&h, &ids, ids.end);
    assert!((end - 50.0).abs() < 1.0, "End at the middle of 8 characters: {end}");
    assert_eq!(value(&h, &ids, ids.start), 0.0, "Start stays");
    let layer = h.state().session.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().clone();
    assert_eq!(layer.props.prop("transform/position").unwrap().value, position, "the layer didn't move");
    assert_eq!(h.state().session.history.undo.len(), undo + 1, "the drag is one undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(value(&h, &ids, ids.end), 100.0, "one undo puts it back");
}

/// End animated from 0% (0 s) to 100% (2 s): at 1 s the bar sits at 50%, and dragging it adds a
/// keyframe at 1 s with the new value; the other keys stay.
#[test]
fn dragging_an_animated_end_keys_the_current_time() {
    let (mut s, ids) = setup();
    s.execute("prop.addKey", json!({"layer": ids.layer, "prop": ids.end, "time": 0.0, "value": 0.0})).unwrap();
    s.execute("prop.addKey", json!({"layer": ids.layer, "prop": ids.end, "time": 2.0, "value": 100.0})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.state.selected_props = vec![(LayerId(ids.layer), ids.start)];
    let mut h = harness(s);
    let at = handle(&h, ids.end).expect("an End bar (selecting Start shows the selector)");
    assert!((at.x - caret_x(&h, &ids, 4)).abs() < 12.0, "at 1 s End is 50%: the bar is after the fourth character");
    let dx = caret_x(&h, &ids, 6) - caret_x(&h, &ids, 4);
    drag(&mut h, at, at + vec2(dx, 0.0));
    let layer = h.state().session.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().clone();
    let keys: Vec<(f64, f64)> = layer.props.find(ids.end).unwrap().keys.iter().map(|k| (k.time.seconds(), k.value.as_f64())).collect();
    assert_eq!(keys.len(), 3, "{keys:?}");
    // (Times on the comp's frames: 29.97 fps puts "1 s" at 1.001 s.)
    let at = |k: (f64, f64), t: f64, v: f64| (k.0 - t).abs() < 0.01 && (k.1 - v).abs() < 1.0;
    assert!(at(keys[0], 0.0, 0.0) && at(keys[2], 2.0, 100.0), "the other keys stay: {keys:?}");
    assert!(at(keys[1], 1.0, 75.0), "a key at 1 s at 75%: {keys:?}");
}

/// The bars show while the selector, its animator or a property in the animator is selected,
/// and only with the Selection tool and layer controls on.
#[test]
fn bars_show_only_for_a_selected_selector_or_animator() {
    let (s, ids) = setup();
    let h = harness(s);
    assert!(handle(&h, ids.end).is_none(), "only the layer is selected");
    let (mut s, ids) = setup();
    let position = s.active_comp().unwrap().layer(LayerId(ids.layer)).unwrap().props.prop("transform/position").unwrap().uid;
    s.state.selected_props = vec![(LayerId(ids.layer), position)];
    let h = harness(s);
    assert!(handle(&h, ids.end).is_none(), "a property outside the animator is selected");
    let (mut s, ids) = setup();
    s.state.selected_props = vec![(LayerId(ids.layer), ids.color)];
    let mut h = harness(s);
    assert!(handle(&h, ids.end).is_some(), "Fill Color, in the animator, is selected");
    h.state_mut().ui.viewer.show_layer_controls = false;
    h.run_steps(3);
    assert!(handle(&h, ids.end).is_none(), "View ▸ Show Layer Controls off hides them");
    h.state_mut().ui.viewer.show_layer_controls = true;
    h.state_mut().ui.tool = effectcraft_ui_egui::state::Tool::Pen;
    h.run_steps(3);
    assert!(handle(&h, ids.end).is_none(), "only the Selection tool drags them");
}

/// Offset 25%: the Start bar sits two characters in, where the range begins, and dragging it to
/// the boundary after the fourth character writes Start = 50% − Offset, leaving Offset alone.
#[test]
fn bars_include_the_offset_and_a_drag_leaves_it() {
    let (mut s, ids) = setup();
    let offset = animator(&s, ids.layer).sub("selectors").unwrap().groups().next().unwrap().get("offset").unwrap().uid;
    s.execute("prop.set", json!({"layer": ids.layer, "prop": offset, "value": 25.0})).unwrap();
    s.state.selected_props = vec![(LayerId(ids.layer), ids.animator)];
    let mut h = harness(s);
    let at = handle(&h, ids.start).expect("a Start bar");
    assert!((at.x - caret_x(&h, &ids, 2)).abs() < 12.0, "Start 0% + Offset 25%: after the second character");
    let dx = caret_x(&h, &ids, 4) - caret_x(&h, &ids, 2);
    drag(&mut h, at, at + vec2(dx, 0.0));
    assert!((value(&h, &ids, ids.start) - 25.0).abs() < 1.0, "Start {}", value(&h, &ids, ids.start));
    assert_eq!(value(&h, &ids, offset), 25.0, "Offset stays");
}

/// Small text: Start at 0% sits within the reach of the bounding box's left-side scale handle. A
/// press on the handle still scales the layer and leaves Start alone.
#[test]
fn a_bar_on_a_box_handle_leaves_it_the_press() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "duration": 4})).unwrap();
    let l = s.execute("layer.newText", json!({"text": "abcdefgh", "size": 12})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addTextAnimator", json!({"layer": l, "property": "fillColor"})).unwrap();
    let a = animator(&s, l);
    let start = a.sub("selectors").unwrap().groups().next().unwrap().get("start").unwrap().uid;
    s.state.selected_layers = vec![LayerId(l)];
    s.state.selected_props = vec![(LayerId(l), a.uid)];
    let mut h = harness(s);
    let bar = handle(&h, start).expect("a Start bar");
    let at = (0..8)
        .filter_map(|i| h.state().auto.find(&format!("viewer.handle.{l}.{i}")).map(|e| pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)))
        .min_by(|p, q| (*p - bar).length().total_cmp(&(*q - bar).length()))
        .expect("the box's handles");
    assert!((bar - at).length() < 9.0, "the fixture puts Start within the handle's reach: {bar:?} vs {at:?}");
    let scale = |h: &Harness<'_, EffectcraftApp>| {
        h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop("transform/scale").unwrap().value.clone()
    };
    let before = scale(&h);
    drag(&mut h, at, at - vec2(40.0, 0.0));
    let layer = h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    assert_eq!(layer.props.find(start).unwrap().value.as_f64(), 0.0, "Start stays");
    assert_ne!(scale(&h), before, "the handle scaled the layer");
}

/// The text the Timeline paints in property `uid`'s value field.
fn timeline_value(h: &Harness<'_, EffectcraftApp>, uid: u64) -> String {
    let e = h.state().auto.find(&format!("timeline.prop.{uid}.value")).unwrap_or_else(|| panic!("timeline.prop.{uid}.value isn't shown"));
    let field = egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]));
    h.output()
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Text(t) if field.contains(t.visual_bounding_rect().center()) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect()
}

/// The centre of the first painted text that reads `label` exactly (a popup's option).
fn text_at(h: &Harness<'_, EffectcraftApp>, label: &str) -> Option<Pos2> {
    h.output().shapes.iter().find_map(|c| match &c.shape {
        egui::Shape::Text(t) if t.galley.text() == label => Some(t.visual_bounding_rect().center()),
        _ => None,
    })
}

fn click_at(h: &mut Harness<'_, EffectcraftApp>, p: Pos2) {
    h.event(Event::PointerMoved(p));
    h.step();
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// #1814: with Units = Index the Timeline shows the Range Selector's Start, End and Offset as
/// counts, without the % sign they carry with Units = Percentage. Units is changed the way a
/// user changes it: its Timeline popup, then the Index option.
#[test]
fn index_units_show_start_end_and_offset_without_a_percent_sign() {
    let (s, ids) = setup();
    let l = LayerId(ids.layer);
    let layer = s.active_comp().unwrap().layer(l).unwrap().clone();
    let text = layer.props.sub("text").unwrap();
    let anim = animator(&s, ids.layer);
    let selectors = anim.sub("selectors").unwrap();
    let sel = selectors.groups().next().unwrap();
    let (offset, units) = (sel.get("offset").unwrap().uid, sel.prop("advanced/units").unwrap().uid);
    let advanced = sel.sub("advanced").unwrap().uid;
    let mut h = harness(s);
    let tl = &mut h.state_mut().ui.timeline;
    tl.open_layers.insert(ids.layer);
    let animators = text.sub("animators").unwrap().uid;
    tl.open_groups.extend([text.uid, animators, anim.uid, selectors.uid, sel.uid, advanced]);
    h.run_steps(3);
    for uid in [ids.start, ids.end, offset] {
        assert!(timeline_value(&h, uid).ends_with('%'), "Percentage units: {}", timeline_value(&h, uid));
    }
    // Units ▸ Index through the Timeline: open its popup, pick the option.
    let units_value = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layer(l).unwrap().props.find(units).unwrap().value.clone();
    assert_eq!(units_value(&h), effectcraft_engine::keyframe::Value::Enum(0), "Percentage to start with");
    let field = h
        .state()
        .auto
        .find(&format!("timeline.prop.{units}.value"))
        .map(|e| pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0))
        .expect("Units is shown");
    click_at(&mut h, field);
    let index = text_at(&h, "Index").expect("the Units popup lists Index");
    click_at(&mut h, index);
    assert_eq!(units_value(&h), effectcraft_engine::keyframe::Value::Enum(1), "the popup set Units to Index");
    for uid in [ids.start, ids.end, offset] {
        let v = timeline_value(&h, uid);
        assert!(!v.is_empty() && !v.contains('%'), "Index units: {v:?}");
    }
}
