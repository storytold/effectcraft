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
fn harness() -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Box", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
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
