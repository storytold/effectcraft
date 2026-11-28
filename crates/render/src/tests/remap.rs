//! Time remapping and separated Position dimensions.

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemKind, LayerSource, Node, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::render_frame;

fn s(x: f64) -> Tick {
    Tick::from_seconds_f64(x)
}

#[test]
fn separated_position_drives_the_transform() {
    let mut p = Project::default();
    let comp = Comp::new(200, 100, FrameRate::FPS_30, s(2.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 20, height: 20, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "S", LayerSource::Solid { item: sid }, (20, 20), None);
    let mut next = p.next_id;
    let mut ids = Ids(&mut next);
    let mut px = ids.prop("positionX", "X Position", Value::Scalar(20.0));
    px.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(20.0)), Keyframe::new(s(1.0), Value::Scalar(180.0))];
    let py = ids.prop("positionY", "Y Position", Value::Scalar(50.0));
    let pz = ids.prop("positionZ", "Z Position", Value::Scalar(0.0));
    p.next_id = next;
    let tr = l.props.sub_mut("transform").unwrap();
    // The combined property is ignored while separated.
    tr.get_mut("position").unwrap().value = Value::Vec3([100.0, 10.0, 0.0]);
    tr.children.extend([Node::Prop(px), Node::Prop(py), Node::Prop(pz)]);
    p.comp_mut(cid).unwrap().layers.push(l);
    let a = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!(a.get(20, 50)[3] > 0.99 && a.get(100, 10)[3] < 0.01);
    let b = render_frame(&p, cid, s(0.5), 1.0);
    assert!(b.get(100, 50)[3] > 0.99 && b.get(20, 50)[3] < 0.01);
}

#[test]
fn time_remap_picks_the_source_frame() {
    let mut p = Project::default();
    let outer = Comp::new(100, 100, FrameRate::FPS_30, s(4.0));
    let mut inner = Comp::new(100, 100, FrameRate::FPS_30, s(4.0));
    // Inner comp: a solid that moves from x=10 (t=0) to x=90 (t=2).
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 10, height: 10, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &inner, "S", LayerSource::Solid { item: sid }, (10, 10), None);
    l.props.prop_mut("transform/position").unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Vec3([10.0, 50.0, 0.0])), Keyframe::new(s(2.0), Value::Vec3([90.0, 50.0, 0.0]))];
    inner.layers.push(l);
    let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
    let cid = p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(outer.clone().into()));
    let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (100, 100), None);
    let mut next = p.next_id;
    // Freeze on source time 2 s.
    let tr = Ids(&mut next).prop("timeRemap", "Time Remap", Value::Scalar(2.0));
    p.next_id = next;
    let mut tr = tr;
    tr.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(2.0)).hold()];
    pre.props.children.push(Node::Prop(tr));
    p.comp_mut(cid).unwrap().layers.push(pre);
    let img = render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!(img.get(90, 50)[3] > 0.99, "frozen at the end position");
    assert!(img.get(10, 50)[3] < 0.01);
}

#[test]
fn layer_span_is_the_source_duration_in_layer_time() {
    let mut p = Project::default();
    let outer = Comp::new(100, 100, FrameRate::FPS_30, s(4.0));
    let inner = Comp::new(100, 100, FrameRate::FPS_30, s(2.5));
    let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 10, height: 10, pixel_aspect: 1.0 }));
    let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (100, 100), None);
    let sol = build::layer(&mut p, &outer, "S", LayerSource::Solid { item: sid }, (10, 10), None);
    let (first, end) = crate::eval::layer_span(&p, &pre).expect("a nested comp has a duration");
    assert!(first.abs() < 1e-9 && (end - 2.5).abs() < 1e-9);
    assert_eq!(crate::eval::layer_span(&p, &sol), None, "solids have no duration");
    // Time Remap: layer time is not source time any more, the visible span is reported.
    let mut next = p.next_id;
    let tr = Ids(&mut next).prop("timeRemap", "Time Remap", Value::Scalar(0.0));
    p.next_id = next;
    pre.props.children.push(Node::Prop(tr));
    let (first, end) = crate::eval::layer_span(&p, &pre).expect("a remapped layer has a visible span");
    assert!(end > first);
}

/// After Effects keeps Time Remap values as float32 seconds: a remap keyed onto a cut at 0.24 s
/// reads 0.2399999946 s, and After Effects still shows the layer that starts at the cut. A
/// remapped time that falls short of a nested in point by under about 1/1500 of a nested frame
/// reaches it (After Effects 26.5 lab: 0.23999 shows the layer after a 30 fps cut at 0.2400065,
/// 0.23998 the one before; 0.24999 / 0.24998 at 60 fps and 0.24998 / 0.24995 at 24 fps around
/// 0.25). Before, the precomp was drawn just before the cut.
#[test]
fn a_remapped_time_just_short_of_a_cut_reaches_it() {
    // (nested frame rate, cut, remapped value, the layer after the cut shows)
    let cases = [
        (FrameRate::FPS_30, 0.24000651041667, f64::from(0.24_f32), true),
        (FrameRate::FPS_30, 0.24000651041667, f64::from(0.23999_f32), true),
        (FrameRate::FPS_30, 0.24000651041667, f64::from(0.23998_f32), false),
        (FrameRate::FPS_30, 0.24000651041667, f64::from(0.2399_f32), false),
        (FrameRate::FPS_60, 0.25, f64::from(0.24999_f32), true),
        (FrameRate::FPS_60, 0.25, f64::from(0.24998_f32), false),
        (FrameRate::FPS_24, 0.25, f64::from(0.24998_f32), true),
        (FrameRate::FPS_24, 0.25, f64::from(0.24995_f32), false),
        (FrameRate::FPS_30, 0.25511068, 0.25511068 - 1e-5, true),
        (FrameRate::FPS_30, 0.25511068, 0.25511068 - 5e-5, false),
    ];
    for (rate, cut, v, after) in cases {
        let mut p = Project::default();
        let outer = Comp::new(10, 10, FrameRate::FPS_30, s(2.0));
        let mut inner = Comp::new(10, 10, rate, s(2.0));
        let mut solid = |color: [f32; 3], from: Tick, to: Tick| {
            let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color, width: 10, height: 10, pixel_aspect: 1.0 }));
            let mut l = build::layer(&mut p, &inner, "S", LayerSource::Solid { item: sid }, (10, 10), None);
            (l.in_point, l.out_point) = (from, to);
            l
        };
        let before = solid([1.0, 0.0, 0.0], Tick::ZERO, s(cut));
        let later = solid([0.0, 1.0, 0.0], s(cut), s(2.0));
        inner.layers = vec![later, before].into();
        let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let cid = p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(outer.clone().into()));
        let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (10, 10), None);
        let mut next = p.next_id;
        let mut tr = Ids(&mut next).prop("timeRemap", "Time Remap", Value::Scalar(0.0));
        p.next_id = next;
        let frame = FrameRate::FPS_30.tick_of(1);
        tr.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), Keyframe::new(frame, Value::Scalar(v)), Keyframe::new(s(1.0), Value::Scalar(1.0))];
        pre.props.children.push(Node::Prop(tr));
        p.comp_mut(cid).unwrap().layers.push(pre);
        let px = render_frame(&p, cid, frame, 1.0).get(5, 5);
        let (green, red) = (px[1] > 0.5 && px[0] < 0.5, px[0] > 0.5 && px[1] < 0.5);
        assert!(px[3] > 0.99 && green == after && red != after, "{rate:?} cut {cut} remapped to {v}: {px:?}");
    }
}

/// The nudge above stops at the nested comp's end: a remapped time in its last 1/1500 frame
/// still draws the nested comp's last frame instead of nothing.
#[test]
fn a_remapped_time_just_short_of_the_nested_end_still_draws() {
    let mut p = Project::default();
    let outer = Comp::new(10, 10, FrameRate::FPS_30, s(2.0));
    let mut inner = Comp::new(10, 10, FrameRate::FPS_30, s(1.0));
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 10, height: 10, pixel_aspect: 1.0 }));
    inner.layers.push(build::layer(&mut p, &inner, "S", LayerSource::Solid { item: sid }, (10, 10), None));
    let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
    let cid = p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(outer.clone().into()));
    let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (10, 10), None);
    let mut next = p.next_id;
    let mut tr = Ids(&mut next).prop("timeRemap", "Time Remap", Value::Scalar(0.0));
    p.next_id = next;
    // 1 s minus 1e-5 s: inside the nested comp, and closer to its end than the nudge (1/45 000 s).
    tr.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(1.0 - 1e-5)).hold()];
    pre.props.children.push(Node::Prop(tr));
    p.comp_mut(cid).unwrap().layers.push(pre);
    let px = render_frame(&p, cid, Tick::ZERO, 1.0).get(5, 5);
    assert!(px[3] > 0.99 && px[0] > 0.5, "the nested comp's last frame: {px:?}");
}
