//! The key marquee in the Timeline (#2857): a box selects a key as soon as it touches any part of
//! the key's glyph, not when it reaches the key's center.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, Pos2, Vec2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// How far a linear key's diamond reaches from its center, without its hairline (0.25 pt).
const REACH: f32 = 5.5;

/// A comp with one solid, its Opacity keys at 1 s and 2 s (linear) revealed.
fn harness_for(duration: f64) -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Box", "width": 320, "height": 180, "frameRate": 30, "duration": duration})).unwrap();
    let id = s.execute("layer.newSolid", json!({"name": "L", "color": "#e04020", "width": 80, "height": 80})).unwrap()["layer"].as_u64().unwrap();
    for (t, v) in [(1.0, 0.0), (2.0, 100.0)] {
        s.execute("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": t, "value": v})).unwrap();
    }
    s.execute("layer.select", json!({"layers": [id]})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.opacity", json!({})).unwrap();
    h.run_steps(4);
    (h, id)
}

fn harness() -> (Harness<'static, EffectcraftApp>, u64) {
    harness_for(4.0)
}

/// The center of the key at `secs`: the middle of its hit rect, which is centered on the glyph.
fn key_center(h: &Harness<'_, EffectcraftApp>, id: u64, secs: f64) -> Pos2 {
    let comp = h.state().session.active_comp().unwrap();
    let uid = comp.layer(LayerId(id)).unwrap().props.prop("transform/opacity").unwrap().uid;
    let key = format!("timeline.keyAt.{uid}.{}", Tick::from_seconds_f64(secs).0);
    let e = h.state().auto.find(&key).unwrap_or_else(|| panic!("no {key}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

/// Draw a marquee from `from` to `to`, then report the selected keys' times in seconds.
fn marquee(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) -> Vec<f64> {
    let modifiers = Modifiers::default();
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for i in 1..=24 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 24.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(2);
    h.state().session.state.selected_keys.iter().map(|k| k.time.seconds()).collect()
}

#[test]
fn a_box_selects_a_key_it_grazes_from_any_side_and_not_one_it_stops_short_of() {
    let (mut h, id) = harness();
    let c = key_center(&h, id, 1.0);
    // Boxes end `gap` from the key's center, coming from the right, the left and below (the row
    // above holds the layer bar, where a press would drag the layer).
    let sides: [(&str, fn(f32) -> (Vec2, Vec2)); 3] = [
        ("right", |gap| (vec2(30.0, 5.0), vec2(gap, -5.0))),
        ("left", |gap| (vec2(-30.0, 5.0), vec2(-gap, -5.0))),
        ("below", |gap| (vec2(5.0, 40.0), vec2(-5.0, gap))),
    ];
    for (side, gap_to_box) in sides {
        let (from, to) = gap_to_box(REACH);
        assert_eq!(marquee(&mut h, c + from, c + to), [1.0], "a box touching the glyph's {side} point selects the key");
        // Clear of the glyph and its hairline by 1 pt.
        let (from, to) = gap_to_box(REACH + 0.25 + 1.0);
        assert!(marquee(&mut h, c + from, c + to).is_empty(), "a box 1 pt short on the {side} misses it");
        assert!(h.state().session.state.selected_layers.is_empty(), "a box that catches nothing deselects (layers included)");
    }
}

/// #3056: one box spans the layer outline and the time graph. Dragged from the empty outline
/// into the graph, or from the graph back over the outline, it selects the layers whose rows it
/// crosses and the keys inside it, through the real drag, release and selection commands.
#[test]
fn a_box_dragged_across_the_outline_and_the_graph_selects_rows_and_keys() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Box", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let mut ids = vec![];
    for name in ["A", "B"] {
        let id = s.execute("layer.newSolid", json!({"name": name, "color": "#e04020", "width": 80, "height": 80})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": 1.0, "value": 50})).unwrap();
        ids.push(id);
    }
    s.execute("layer.select", json!({"layers": ids})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.opacity", json!({})).unwrap();
    h.run_steps(4);
    h.state_mut().session.execute("edit.deselectAll", json!({})).unwrap();
    h.run_steps(2);
    // B was added last, so its rows are on top; below A's rows the outline is empty.
    let (top, bottom) = (key_center(&h, ids[1], 1.0), key_center(&h, ids[0], 1.0));
    let outline = h.state().auto.find("timeline.layerMarquee").expect("the outline's empty area").rect;
    let below = pos2(outline[0] + 60.0, bottom.y + 40.0);
    assert!(below.x < top.x && below.y < outline[1] + outline[3], "the start is in the outline, below the rows");
    let selected = |h: &Harness<'_, EffectcraftApp>| {
        let st = &h.state().session.state;
        let mut layers: Vec<u64> = st.selected_layers.iter().map(|l| l.0).collect();
        let mut keyed: Vec<u64> = st.selected_keys.iter().map(|k| k.layer.0).collect();
        layers.sort_unstable();
        keyed.sort_unstable();
        (layers, keyed)
    };
    let mut both = ids.clone();
    both.sort_unstable();
    // From the outline up and right into the graph, over both keys.
    let keys = marquee(&mut h, below, top + vec2(15.0, -3.0));
    assert_eq!(keys, [1.0, 1.0]);
    assert_eq!(selected(&h), (both.clone(), both.clone()), "outline → graph");
    h.state_mut().session.execute("edit.deselectAll", json!({})).unwrap();
    h.run_steps(2);
    // And back: from beside the top key in the graph down and left over the outline.
    let keys = marquee(&mut h, top + vec2(30.0, 0.0), below);
    assert_eq!(keys, [1.0, 1.0]);
    assert_eq!(selected(&h), (both.clone(), both), "graph → outline");
}

/// Thirty expanded layers force row virtualization and leave room for wheel and edge scrolling.
fn rows_harness() -> (Harness<'static, EffectcraftApp>, Vec<u64>) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Rows", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let mut ids = vec![];
    for i in 0..30 {
        let id = s.execute("layer.newSolid", json!({"name": format!("Layer {i}"), "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": 1.0, "value": 50})).unwrap();
        ids.push(id);
    }
    s.execute("layer.select", json!({"layers": ids})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.opacity", json!({})).unwrap();
    h.run_steps(4);
    (h, ids)
}

/// The originally enclosed key stays in the box when wheel scrolling takes its row offscreen.
#[test]
fn scrolling_rows_during_a_marquee_keeps_offscreen_keys_in_the_box() {
    let (mut h, ids) = rows_harness();
    let top = *ids.last().unwrap();
    let c = key_center(&h, top, 1.0);
    let from = c - vec2(20.0, 5.0);
    let to = c + vec2(20.0, 30.0);
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerMoved(to));
    h.run_steps(2);
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -400.0),
        modifiers: Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run_steps(20);
    assert!(h.state().ui.timeline.scroll_y > 300.0, "the real wheel handler scrolled rows");
    h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    let selected: Vec<_> = h.state().session.state.selected_keys.iter().map(|k| k.layer.0).collect();
    assert!(selected.contains(&top), "the key on layer {top} scrolled offscreen but remains inside the content-anchored marquee; actual layers {selected:?}");
}

fn press(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerMoved(to));
    h.run_steps(2);
}

fn release(h: &mut Harness<'_, EffectcraftApp>, to: Pos2) {
    h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

fn wheel(h: &mut Harness<'_, EffectcraftApp>, delta: Vec2) {
    h.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        modifiers: Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run_steps(20);
}

fn times(h: &Harness<'_, EffectcraftApp>) -> Vec<f64> {
    h.state().session.state.selected_keys.iter().map(|k| k.time.seconds()).collect()
}

/// Panning time retains hidden keys while both painting and row selection respect the graph boundary.
#[test]
fn horizontal_scroll_keeps_hidden_keys_without_selecting_outline_layers() {
    let (mut h, id) = harness_for(20.0);
    h.state_mut().ui.timeline.pps = Some(200.0);
    h.run_steps(3);
    let c = key_center(&h, id, 1.0);
    let to = c + vec2(20.0, 5.0);
    press(&mut h, c - vec2(20.0, 5.0), to);
    wheel(&mut h, vec2(-400.0, 0.0));
    assert!(h.state().ui.timeline.start > 1.5, "time panned: {}", h.state().ui.timeline.start);
    let graph_left = h.state().auto.find("timeline.ruler").unwrap().rect[0];
    let box_shapes: Vec<_> =
        h.output().shapes.iter().filter(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.rect.max == to && (r.rect.height() - 10.0).abs() < 0.01)).collect();
    assert!(!box_shapes.is_empty(), "the held marquee is drawn");
    assert!(box_shapes.iter().all(|s| s.clip_rect.min.x >= graph_left), "a graph-only box cannot paint over the outline when its anchor scrolls out");
    release(&mut h, to);
    assert_eq!(times(&h), [1.0, 2.0]);
    // Keys select their owning layer, but panning the anchor under the outline must not select
    // unrelated layer rows. The separate cross-column test below covers those rows.
    assert_eq!(h.state().session.state.selected_layers, [LayerId(id)]);
}

/// Returning the viewport shrinks the box again without editing the document or its history.
#[test]
fn shrinking_after_scrolling_removes_keys_again_and_selection_does_not_edit_the_project() {
    let (mut h, id) = harness_for(20.0);
    h.state_mut().ui.timeline.pps = Some(200.0);
    h.run_steps(3);
    let before = h.state().session.project.to_json();
    let revision = h.state().session.revision;
    let undo = h.state().session.history.undo.len();
    let c = key_center(&h, id, 1.0);
    let to = c + vec2(20.0, 5.0);
    press(&mut h, c - vec2(20.0, 5.0), to);
    wheel(&mut h, vec2(-400.0, 0.0));
    wheel(&mut h, vec2(400.0, 0.0));
    assert!(h.state().ui.timeline.start < 0.01);
    release(&mut h, to);
    assert_eq!(times(&h), [1.0], "shrinking is not a union of every key passed");
    assert_eq!(h.state().session.project.to_json(), before);
    assert_eq!(h.state().session.revision, revision);
    assert_eq!(h.state().session.history.undo.len(), undo);
}

/// Invalidated gestures leave the prior selection intact even if the pointer keeps moving.
#[test]
fn cancellation_preserves_selection_and_does_not_restart_until_a_new_press() {
    for cancel in ["escape", "ownership", "focus", "layout", "resize", "edit", "comp"] {
        let (mut h, id) = harness();
        h.state_mut().session.execute("keys.select", json!({"keys": [{"layer":id,"path":"transform/opacity","time":2.0}]})).unwrap();
        h.run_steps(2);
        let c = key_center(&h, id, 1.0);
        let to = c + vec2(10.0, 5.0);
        press(&mut h, c - vec2(20.0, 5.0), to);
        assert_eq!(times(&h), [2.0], "selection commits only on release");
        match cancel {
            "escape" => h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }),
            "ownership" => h.ctx.set_dragged_id(egui::Id::unique("another-owner")),
            "focus" => h.input_mut().focused = false,
            "layout" => h.state_mut().ui.timeline.search = "no matching rows".into(),
            "resize" => {
                h.set_size(vec2(1400.0, 900.0));
            }
            "edit" => {
                h.state_mut().session.execute("prop.set", json!({"layer":id,"path":"transform/opacity","value":75})).unwrap();
            }
            "comp" => {
                h.state_mut().session.execute("comp.new", json!({"name":"Other"})).unwrap();
            }
            _ => unreachable!(),
        }
        h.run_steps(2);
        let expected = times(&h);
        h.event(Event::PointerMoved(to + vec2(1.0, 0.0)));
        h.step();
        release(&mut h, to);
        assert_eq!(times(&h), expected, "{cancel} cancelled without applying the box");
        if !matches!(cancel, "comp" | "edit") {
            assert_eq!(times(&h), [2.0], "{cancel} preserves the prior selection");
        }
    }
}

/// A release-frame pointer move takes effect, modifiers add, and locked keys remain excluded.
#[test]
fn release_uses_the_release_position_and_additive_modifiers_and_excludes_locked_keys() {
    for modifiers in [Modifiers::NONE, Modifiers::SHIFT, Modifiers::COMMAND] {
        let (mut h, id) = harness();
        h.state_mut().session.execute("keys.select", json!({"keys": [{"layer":id,"path":"transform/opacity","time":2.0}]})).unwrap();
        h.run_steps(2);
        let c = key_center(&h, id, 1.0);
        press(&mut h, c - vec2(25.0, 5.0), c - vec2(15.0, 5.0));
        h.event(Event::ModifiersChanged(modifiers));
        h.event(Event::PointerMoved(c + vec2(10.0, 5.0)));
        h.event(Event::PointerButton { pos: c + vec2(10.0, 5.0), button: egui::PointerButton::Primary, pressed: false, modifiers });
        h.run_steps(2);
        let mut actual = times(&h);
        actual.sort_by(f64::total_cmp);
        assert_eq!(actual, if modifiers == Modifiers::NONE { vec![1.0] } else { vec![1.0, 2.0] });
        h.state_mut().session.execute("layer.setSwitch", json!({"layers":[id],"switch":"lock","value":true})).unwrap();
        h.event(Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(2);
        assert!(marquee(&mut h, c - vec2(20.0, 5.0), c + vec2(10.0, 5.0)).is_empty(), "locked keys stay excluded");
    }
}

/// Cross-column selection uses logical headers, including those no longer rendered after scrolling.
#[test]
fn crossing_into_the_outline_after_scrolling_selects_offscreen_layer_rows() {
    let (mut h, ids) = rows_harness();
    let c = key_center(&h, *ids.last().unwrap(), 1.0);
    let outline = h.state().auto.find("timeline.layerMarquee").unwrap().rect;
    let to = pos2(outline[0] + 40.0, c.y + 30.0);
    press(&mut h, c + vec2(20.0, -5.0), to);
    wheel(&mut h, vec2(0.0, -400.0));
    assert!(h.state().ui.timeline.scroll_y > 300.0);
    release(&mut h, to);
    let selected = &h.state().session.state.selected_layers;
    assert!(selected.contains(&LayerId(ids[ids.len() - 2])), "the second header is inside the box although it scrolled offscreen");
    assert!(selected.len() > 5, "all intervening rows are selected");
}

/// Edge scrolling continues without pointer motion and stops as soon as the gesture is cancelled.
#[test]
fn a_stationary_pointer_at_either_edge_scrolls_only_until_release_or_cancel() {
    let (mut h, ids) = rows_harness();
    let c = key_center(&h, *ids.last().unwrap(), 1.0);
    let area = h.state().auto.find("timeline.layerMarquee").unwrap().rect;
    let bottom = pos2(c.x + 20.0, area[1] + area[3] - 2.0);
    press(&mut h, c - vec2(20.0, 5.0), bottom);
    let before = h.state().ui.timeline.scroll_y;
    h.run_steps(30);
    assert!(h.state().ui.timeline.scroll_y > before + 50.0, "a stationary drag requests more frames and scrolls");
    let top = pos2(bottom.x, area[1] + 2.0);
    h.event(Event::PointerMoved(top));
    h.step();
    let before = h.state().ui.timeline.scroll_y;
    h.run_steps(10);
    assert!(h.state().ui.timeline.scroll_y < before, "the top edge scrolls back");
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    let before = h.state().ui.timeline.scroll_y;
    h.run_steps(10);
    assert_eq!(h.state().ui.timeline.scroll_y, before, "Escape stops edge scrolling");
    release(&mut h, top);
    assert!(times(&h).is_empty(), "cancelled box selects nothing");
}

/// Batched wheel events cannot move the press anchor or make release query stale row positions.
#[test]
fn wheel_on_press_captures_the_old_anchor_and_wheel_on_release_uses_the_new_rows() {
    for scroll_on_press in [true, false] {
        let (mut h, ids) = rows_harness();
        let top = *ids.last().unwrap();
        let c = key_center(&h, top, 1.0);
        let from = c - vec2(20.0, 5.0);
        let to = c + vec2(20.0, 30.0);
        if scroll_on_press {
            h.event(Event::PointerMoved(from));
            h.step();
            h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        } else {
            press(&mut h, from, to);
        }
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -400.0),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        });
        if !scroll_on_press {
            h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        }
        h.step();
        if scroll_on_press {
            h.event(Event::PointerMoved(to));
            h.run_steps(20);
            release(&mut h, to);
        }
        assert!(h.state().session.state.selected_keys.iter().any(|k| k.layer == LayerId(top)), "scroll_on_press={scroll_on_press}");
    }
}
