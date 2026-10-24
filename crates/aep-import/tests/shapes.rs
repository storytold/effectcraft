//! Shape layers (238-242, 330-333, 451-483, 519): every shape fixture imports without losses with
//! the values it was made with.

mod common;

use common::{comp, load, try_import};
use effectcraft_aep_import::LossCategory;
use effectcraft_keyframe::{Gradient, Value};
use effectcraft_project::{Layer, LayerSource, Project, PropGroup};

fn shape_layer(p: &Project, r: &effectcraft_aep_import::ImportResult) -> Layer {
    let (_, c) = comp(p, r, "C");
    c.layers.iter().find(|l| matches!(l.source, LayerSource::Shape)).cloned().expect("a shape layer")
}

fn lossless(name: &str) -> Layer {
    let mut p = Project::default();
    let r = try_import(&mut p, name, false).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(r.report.imported.shape_layers, 1, "{name}");
    shape_layer(&p, &r)
}

/// The items of the contents group `path` (`""` = the layer's contents), as `(match id, name)`.
fn items(g: &PropGroup) -> Vec<(String, String)> {
    g.children.iter().filter_map(|n| n.as_group()).map(|g| (g.match_id.clone(), g.name.clone())).collect()
}

fn contents(l: &Layer) -> &PropGroup {
    l.props.sub("contents").expect("contents")
}

fn group(g: &PropGroup, i: usize) -> &PropGroup {
    g.children.iter().filter_map(|n| n.as_group()).nth(i).expect("item")
}

fn val(g: &PropGroup, path: &str) -> Value {
    g.prop(path).unwrap_or_else(|| panic!("no {path}")).value.clone()
}

#[test]
fn an_ellipse_with_a_fill_in_a_group() {
    let l = lossless("451-shape-ellipse-400x300-fill.aep");
    let c = contents(&l);
    assert_eq!(items(c), vec![("group".to_string(), "Group 1".to_string())]);
    let g = group(c, 0);
    let inner = g.sub("contents").expect("group contents");
    assert_eq!(items(inner), vec![("ellipse".into(), "Ellipse Path 1".into()), ("fill".into(), "Fill 1".into())]);
    assert_eq!(val(group(inner, 0), "size"), Value::Vec2([400.0, 300.0]));
    assert_eq!(val(group(inner, 0), "position"), Value::Vec2([0.0, 0.0]));
    assert_eq!(val(group(inner, 1), "color"), Value::Color([0.0, 1.0, 0.0, 1.0]));
    assert_eq!(val(group(inner, 1), "opacity"), Value::Scalar(100.0));
    // The layer: anchor at the origin, position the composition centre.
    assert_eq!(val(&l.props, "transform/anchor"), Value::Vec3([0.0, 0.0, 0.0]));
    assert_eq!(val(&l.props, "transform/position"), Value::Vec3([960.0, 540.0, 0.0]));
}

#[test]
fn group_transform_layer_anchor_stroke_trim_rect_star() {
    let inner = |l: &Layer| group(contents(l), 0).sub("contents").cloned().expect("contents");
    let tr = |l: &Layer| group(contents(l), 0).sub("transform").cloned().expect("transform");

    let l = lossless("457-shape-group-scale-50-80.aep");
    assert_eq!(val(&tr(&l), "scale"), Value::Vec2([50.0, 80.0]), "group scale is percent as stored");

    let l = lossless("462-shape-layer-anchor-10-20.aep");
    assert_eq!(val(&l.props, "transform/anchor"), Value::Vec3([10.0, 20.0, 0.0]), "a shape layer's anchor is pixels as stored");

    let l = lossless("464-shape-stroke-opacity-50.aep");
    let s = group(&inner(&l), 2).clone();
    assert_eq!(
        (s.match_id.as_str(), val(&s, "color"), val(&s, "width"), val(&s, "opacity")),
        ("stroke", Value::Color([1.0, 0.0, 0.0, 1.0]), Value::Scalar(6.0), Value::Scalar(50.0))
    );

    let l = lossless("467-shape-trim-end-70.aep");
    let t = group(&inner(&l), 2).clone();
    assert_eq!(
        (t.match_id.as_str(), val(&t, "start"), val(&t, "end"), val(&t, "offset"), val(&t, "mode")),
        ("trim", Value::Scalar(0.0), Value::Scalar(70.0), Value::Scalar(0.0), Value::Enum(0))
    );
    let l = lossless("469-shape-trim-type-2.aep");
    assert_eq!(val(group(&inner(&l), 2), "mode"), Value::Enum(1), "individually");

    let l = lossless("471-shape-rect-roundness-30.aep");
    let r = group(&inner(&l), 2).clone();
    assert_eq!((r.match_id.as_str(), val(&r, "size"), val(&r, "roundness")), ("rect", Value::Vec2([200.0, 100.0]), Value::Scalar(30.0)));

    let l = lossless("473-shape-star-points-6.aep");
    let s = group(&inner(&l), 2).clone();
    assert_eq!(
        (s.match_id.as_str(), val(&s, "type"), val(&s, "points"), val(&s, "outerRadius"), val(&s, "innerRadius"), val(&s, "rotation")),
        ("star", Value::Enum(0), Value::Scalar(6.0), Value::Scalar(100.0), Value::Scalar(50.0), Value::Scalar(0.0))
    );
}

#[test]
fn two_groups_and_a_nested_group() {
    let l = lossless("482-shape-two-groups.aep");
    let c = contents(&l);
    assert_eq!(items(c), vec![("group".into(), "Group 1".into()), ("group".into(), "Group 2".into())]);
    let g2 = group(c, 1).sub("contents").expect("contents").clone();
    assert_eq!(val(group(&g2, 0), "size"), Value::Vec2([200.0, 100.0]));
    assert_eq!(val(group(&g2, 1), "color"), Value::Color([0.0, 0.0, 1.0, 1.0]));

    let l = lossless("483-shape-nested-group-rect.aep");
    let inner = group(contents(&l), 0).sub("contents").expect("contents").clone();
    assert_eq!(items(&inner).iter().map(|i| i.0.as_str()).collect::<Vec<_>>(), vec!["ellipse", "fill", "group"]);
    let nested = group(&inner, 2).sub("contents").expect("nested contents").clone();
    assert_eq!(val(group(&nested, 0), "size"), Value::Vec2([100.0, 50.0]));
}

#[test]
fn every_item_at_its_default() {
    let l = lossless("519-shape-all-items-default.aep");
    let inner = group(contents(&l), 0).sub("contents").expect("contents").clone();
    assert_eq!(
        items(&inner).iter().map(|i| i.0.as_str()).collect::<Vec<_>>(),
        vec!["rect", "ellipse", "star", "fill", "stroke", "gfill", "gstroke", "trim", "round"]
    );
    let g = |i| group(&inner, i).clone();
    assert_eq!((val(&g(0), "size"), val(&g(0), "roundness")), (Value::Vec2([100.0, 100.0]), Value::Scalar(0.0)));
    assert_eq!(val(&g(1), "size"), Value::Vec2([100.0, 100.0]));
    assert_eq!((val(&g(2), "points"), val(&g(2), "innerRadius"), val(&g(2), "outerRadius")), (Value::Scalar(5.0), Value::Scalar(50.0), Value::Scalar(100.0)));
    assert_eq!((val(&g(3), "color"), val(&g(3), "rule")), (Value::Color([1.0, 0.0, 0.0, 1.0]), Value::Enum(0)));
    assert_eq!(
        (val(&g(4), "color"), val(&g(4), "width"), val(&g(4), "cap"), val(&g(4), "join"), val(&g(4), "miter")),
        (Value::Color([1.0; 4]), Value::Scalar(2.0), Value::Enum(0), Value::Enum(0), Value::Scalar(4.0))
    );
    for i in [5, 6] {
        assert_eq!((val(&g(i), "start"), val(&g(i), "end"), val(&g(i), "type")), (Value::Vec2([0.0, 0.0]), Value::Vec2([100.0, 0.0]), Value::Enum(0)));
        assert_eq!(val(&g(i), "colors"), Value::Gradient(Gradient::default()), "white to black");
    }
    assert_eq!((val(&g(7), "start"), val(&g(7), "end")), (Value::Scalar(0.0), Value::Scalar(100.0)));
    assert_eq!(val(&g(8), "radius"), Value::Scalar(10.0));
}

#[test]
fn the_earlier_shape_cases() {
    // 238: rectangle 400x300, green fill, round corners 20.
    let l = lossless("238-shape-rect-rounded-corners.aep");
    let inner = group(contents(&l), 0).sub("contents").expect("contents").clone();
    assert_eq!(items(&inner).iter().map(|i| i.0.as_str()).collect::<Vec<_>>(), vec!["rect", "fill", "round"]);
    assert_eq!(val(group(&inner, 2), "radius"), Value::Scalar(20.0));
    // 239: green stroke 8 px, round cap and join.
    let l = lossless("239-shape-stroke-green-w8-round.aep");
    let inner = group(contents(&l), 0).sub("contents").expect("contents").clone();
    let s = group(&inner, 1);
    assert_eq!(
        (val(s, "width"), val(s, "cap"), val(s, "join"), val(s, "color")),
        (Value::Scalar(8.0), Value::Enum(1), Value::Enum(1), Value::Color([0.0, 1.0, 0.0, 1.0]))
    );
    // 240: the custom path in pixels with its tangents.
    let l = lossless("240-shape-custom-path-tangents.aep");
    let inner = group(contents(&l), 0).sub("contents").expect("contents").clone();
    let Value::Path(p) = val(group(&inner, 0), "path") else { panic!("not a path") };
    let close = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3;
    for (got, want) in p.vertices.iter().zip([[0.0, -100.0], [100.0, 100.0], [-100.0, 100.0]]) {
        assert!(close(*got, want), "{got:?} {want:?}");
    }
    for (got, want) in p.in_tangents.iter().zip([[-10.0, -20.0], [30.0, 20.0], [-60.0, 10.0]]) {
        assert!(close(*got, want), "{got:?} {want:?}");
    }
    for (got, want) in p.out_tangents.iter().zip([[40.0, 0.0], [-20.0, 30.0], [0.0, -50.0]]) {
        assert!(close(*got, want), "{got:?} {want:?}");
    }
    assert!(p.closed);
    // 241 linear, 332 radial gradient fills: points, type.
    let l = lossless("241-shape-gradient-fill-linear.aep");
    let inner = group(contents(&l), 0).sub("contents").expect("contents").clone();
    let gf = group(&inner, 1);
    assert_eq!((val(gf, "start"), val(gf, "end"), val(gf, "type")), (Value::Vec2([-200.0, 0.0]), Value::Vec2([200.0, 0.0]), Value::Enum(0)));
    let l = lossless("332-shape-gradient-fill-radial.aep");
    let inner = group(contents(&l), 0).sub("contents").expect("contents").clone();
    let gf = group(&inner, 1);
    assert_eq!((val(gf, "start"), val(gf, "end"), val(gf, "type")), (Value::Vec2([0.0, 0.0]), Value::Vec2([200.0, 0.0]), Value::Enum(1)));
    // 242: group position.
    let l = lossless("242-shape-group-position-50-50.aep");
    assert_eq!(val(group(contents(&l), 0).sub("transform").expect("transform"), "position"), Value::Vec2([50.0, 50.0]));
    // 330 star, 331 polygon.
    let l = lossless("330-shape-polystar-star-5-50-100.aep");
    let s = group(group(contents(&l), 0).sub("contents").expect("contents"), 0).clone();
    assert_eq!(
        (val(&s, "type"), val(&s, "points"), val(&s, "innerRadius"), val(&s, "outerRadius")),
        (Value::Enum(0), Value::Scalar(5.0), Value::Scalar(50.0), Value::Scalar(100.0))
    );
    let l = lossless("331-shape-polystar-polygon-5-100.aep");
    let s = group(group(contents(&l), 0).sub("contents").expect("contents"), 0).clone();
    assert_eq!((val(&s, "type"), val(&s, "outerRadius")), (Value::Enum(1), Value::Scalar(100.0)));
    assert!(s.prop("innerRadius").is_none(), "a polygon has no inner radius");
    // 333: two rectangles, the second at its default size with a position.
    let l = lossless("333-shape-group-two-rects.aep");
    let inner = group(contents(&l), 0).sub("contents").expect("contents").clone();
    assert_eq!(items(&inner).iter().map(|i| i.0.as_str()).collect::<Vec<_>>(), vec!["rect", "rect", "fill"]);
    assert_eq!((val(group(&inner, 1), "size"), val(group(&inner, 1), "position")), (Value::Vec2([100.0, 100.0]), Value::Vec2([300.0, 0.0])));
}

#[test]
fn shape_layers_are_no_longer_skipped() {
    let (p, r) = load("238-shape-rect-rounded-corners.aep");
    assert_eq!(r.report.count(LossCategory::ShapeLayers), 0);
    let (_, c) = comp(&p, &r, "C");
    assert_eq!(c.layers.len(), 2);
}
