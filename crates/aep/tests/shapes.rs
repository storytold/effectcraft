//! Shape layers: rectangle, fill, stroke, rounded corners, custom path, gradient fill and group
//! position, checked against the values the corpus was made with (its MANIFEST, batch 2). The shape
//! layer `S` is on top of the solid `L`; its group is `Group 1`.

mod common;

use common::{assert_near, first_comp, fixture, layer_named, open};
use effectcraft_aep::{LayerType, Project, ShapeGroup, ShapeItem, StrokeCap, StrokeJoin, ValueSource};

fn groups(name: &str) -> Vec<ShapeGroup> {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_named(&first_comp(&project), "S");
    let h = layer.header().unwrap();
    assert_eq!(h.layer_type, LayerType::Shape, "{name}");
    assert!(h.collapses_transformations(), "{name}: shape layers have the collapse switch from creation");
    layer.shape_groups()
}

fn only_group(name: &str) -> ShapeGroup {
    let mut g = groups(name);
    assert_eq!(g.len(), 1, "{name}");
    g.remove(0)
}

fn rect_size(item: &ShapeItem) -> [f64; 2] {
    let ShapeItem::Rect { name, size } = item else { panic!("{item:?}") };
    assert_eq!(name.as_deref(), Some("Rectangle Path 1"));
    let size = size.expect("rect size");
    assert_eq!(size.source, ValueSource::Stored);
    size.value
}

#[test]
fn rectangle_fill_and_rounded_corners() {
    // 238: Rectangle Path 400x300, Fill [0,1,0], Round Corners radius 20 (in this order).
    let g = only_group("238-shape-rect-rounded-corners.aep");
    assert_eq!(g.name.as_deref(), Some("Group 1"));
    assert_eq!((g.position.value, g.position.source), ([0.0, 0.0], ValueSource::Default));
    assert_eq!(g.items.len(), 3);
    assert_eq!(rect_size(&g.items[0]), [400.0, 300.0]);
    let ShapeItem::Fill { name, color } = &g.items[1] else { panic!("{:?}", g.items[1]) };
    assert_eq!(name.as_deref(), Some("Fill 1"));
    assert_eq!(color.unwrap().value, [1.0, 0.0, 1.0, 0.0]);
    let ShapeItem::RoundCorners { name, radius } = &g.items[2] else { panic!("{:?}", g.items[2]) };
    assert_eq!(name.as_deref(), Some("Round Corners 1"));
    assert_eq!(radius.unwrap().value, 20.0);
}

#[test]
fn stroke_colour_width_cap_and_join() {
    // 239: Stroke [0,1,0] width 8, Line Cap 2 (round), Line Join 2 (round); no Fill.
    let g = only_group("239-shape-stroke-green-w8-round.aep");
    assert_eq!(g.items.len(), 2);
    assert_eq!(rect_size(&g.items[0]), [400.0, 300.0]);
    let ShapeItem::Stroke { name, stroke } = &g.items[1] else { panic!("{:?}", g.items[1]) };
    assert_eq!(name.as_deref(), Some("Stroke 1"));
    assert_eq!(stroke.color.unwrap().value, [1.0, 0.0, 1.0, 0.0]);
    assert_eq!(stroke.width.unwrap().value, 8.0);
    assert_eq!((stroke.cap.value, stroke.cap.source), (StrokeCap::Round, ValueSource::Stored));
    assert_eq!((stroke.join.value, stroke.join.source), (StrokeJoin::Round, ValueSource::Stored));
    // Touching the join stores the miter limit in its default state (4).
    assert_eq!(stroke.miter_limit.unwrap().value, 4.0);
    assert!(!g.items.iter().any(|i| matches!(i, ShapeItem::Fill { .. })));
}

#[test]
fn custom_path_with_tangents_in_pixels() {
    // 240: Path 1, closed, vertices [0,-100] [100,100] [-100,100], inTangents [-10,-20] [30,20] [-60,10],
    // outTangents [40,0] [-20,30] [0,-50] (the box of vertices and handles is -160,-120,130,130), then a Fill.
    let g = only_group("240-shape-custom-path-tangents.aep");
    assert_eq!(g.items.len(), 2);
    let ShapeItem::Path { name, paths } = &g.items[0] else { panic!("{:?}", g.items[0]) };
    assert_eq!(name.as_deref(), Some("Path 1"));
    assert_eq!(paths.len(), 1);
    let p = &paths[0];
    assert!(!p.open);
    assert_near(&p.bbox.map(f64::from), &[-160.0, -120.0, 130.0, 130.0], 1e-6);
    let v = p.vertices((1.0, 1.0));
    let positions: Vec<[f64; 2]> = v.iter().map(|x| x.position).collect();
    let in_t: Vec<[f64; 2]> = v.iter().map(|x| x.in_tangent()).collect();
    let out_t: Vec<[f64; 2]> = v.iter().map(|x| x.out_tangent()).collect();
    for (got, want) in positions.iter().zip([[0.0, -100.0], [100.0, 100.0], [-100.0, 100.0]]) {
        assert_near(got, &want, 1e-3);
    }
    for (got, want) in in_t.iter().zip([[-10.0, -20.0], [30.0, 20.0], [-60.0, 10.0]]) {
        assert_near(got, &want, 1e-3);
    }
    for (got, want) in out_t.iter().zip([[40.0, 0.0], [-20.0, 30.0], [0.0, -50.0]]) {
        assert_near(got, &want, 1e-3);
    }
    assert!(matches!(g.items[1], ShapeItem::Fill { .. }));
}

#[test]
fn gradient_fill_start_and_end() {
    // 241: Gradient Fill 1 (linear), start [-200,0], end [200,0] in pixels; type and stops are not stored.
    let g = only_group("241-shape-gradient-fill-linear.aep");
    assert_eq!(g.items.len(), 2);
    assert_eq!(rect_size(&g.items[0]), [400.0, 300.0]);
    let ShapeItem::GradientFill { name, start, end } = &g.items[1] else { panic!("{:?}", g.items[1]) };
    assert_eq!(name.as_deref(), Some("Gradient Fill 1"));
    assert_eq!(start.unwrap().value, [-200.0, 0.0]);
    assert_eq!(end.unwrap().value, [200.0, 0.0]);
}

#[test]
fn group_position() {
    // 242: Group 1 / Transform / Position [0,0] -> [50,50]; the contents are unchanged.
    let g = only_group("242-shape-group-position-50-50.aep");
    assert_eq!((g.position.value, g.position.source), ([50.0, 50.0], ValueSource::Stored));
    assert_eq!(g.items.len(), 2);
    assert_eq!(rect_size(&g.items[0]), [400.0, 300.0]);
    assert!(matches!(g.items[1], ShapeItem::Fill { .. }));
}

#[test]
fn other_layers_have_no_shape_groups() {
    let bytes = fixture("238-shape-rect-rounded-corners.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let solid = comp.layers().find(|l| l.header().is_some_and(|h| h.layer_type == LayerType::Av)).unwrap();
    assert!(solid.shape_groups().is_empty());
}
