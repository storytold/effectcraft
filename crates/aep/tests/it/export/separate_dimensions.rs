//! A layer whose Position has Separate Dimensions on: X, Y and Z Position are written as
//! After Effects 2026 stores them (`ADBE Position_0/1/2`, the combined record marked separated).

use crate::export::helpers::*;
use effectcraft_aep::export::LossCategory;
use effectcraft_aep::{PropNode, parse};
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::{LayerSource, Node, ParamUi, Project, Property};
use effectcraft_time::FrameRate;

#[test]
fn separated_position_is_written_as_x_y_and_z() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 640, 360, FrameRate::new(25, 1), 2.0, None);
    let s = add_solid(&mut p, "S", 100, 100, [1.0, 0.0, 0.0], None);
    add_layer(&mut p, c, "S", LayerSource::Solid { item: s }, (100, 100));
    let mut next = p.next_id;
    with_layer(&mut p, c, 0, |l| {
        let tr = l.props.sub_mut("transform").expect("transform");
        // The combined Position is stale once the dimensions are separated.
        tr.get_mut("position").expect("position").value = Value::Vec3([10.0, 10.0, 0.0]);
        let mut y = Property::new(next + 1, "positionY", "Y Position", Value::Scalar(0.0)).with_ui(ParamUi::Number);
        y.keys = vec![Keyframe::new(secs(0.0), Value::Scalar(40.0)), Keyframe::new(secs(1.0), Value::Scalar(200.0))];
        let mut z = Property::new(next + 2, "positionZ", "Z Position", Value::Scalar(0.0)).with_ui(ParamUi::Number);
        z.three_d_only = true;
        let x = Property::new(next, "positionX", "X Position", Value::Scalar(300.0)).with_ui(ParamUi::Number);
        let at = tr.children.iter().position(|n| n.match_id() == "position").expect("position") + 1;
        tr.children.splice(at..at, [Node::Prop(x), Node::Prop(y), Node::Prop(z)]);
    });
    next += 3;
    p.next_id = next;
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::TransformProperties), 0, "{:?}", r.report.losses);
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let comp = view.all_items().into_iter().find(|i| i.display_name().as_deref() == Some("C")).expect("C");
    let layer = comp.comp().expect("comp").layers().next().expect("layer");
    let Some(PropNode::Group(tr)) = layer.properties().and_then(|g| g.get("ADBE Transform Group")) else { panic!("transform") };
    let prop = |m: &str| match tr.get(m) {
        Some(PropNode::Property(p)) => p,
        _ => panic!("{m}"),
    };
    assert_eq!(prop("ADBE Position").flags(), Some(0x803));
    let x = prop("ADBE Position_0");
    assert_eq!((x.flags(), x.static_value().and_then(|v| v.first().copied())), (Some(1), Some(300.0)));
    let y = prop("ADBE Position_1");
    assert!(y.is_animated());
    let ys: Vec<_> = y.keys().map(|k| k.keyframes().filter_map(|f| f.value().and_then(|v| v.first().copied())).collect()).unwrap_or_default();
    assert_eq!(ys, [40.0, 200.0]);
    assert_eq!(prop("ADBE Position_2").static_value().and_then(|v| v.first().copied()), Some(0.0));
}
