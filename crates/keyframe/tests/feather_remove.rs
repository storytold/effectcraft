use effectcraft_keyframe::{FeatherPoint, ShapePath};

#[test]
fn removing_first_vertex_of_closed_path_merges_closing_segment() {
    let mut p = ShapePath::rect([0.0, 0.0], 10.0, 10.0);
    let fp = |segment| FeatherPoint { segment, t: 0.5, radius: 1.0, tension: 0.0 };
    p.feather = vec![fp(3), fp(0)];
    p.vertices.remove(0);
    p.in_tangents.remove(0);
    p.out_tangents.remove(0);
    p.feather_after_remove(0);
    let got: Vec<(usize, f64)> = p.feather.iter().map(|f| (f.segment, f.t)).collect();
    assert_eq!(got, vec![(2, 0.25), (2, 0.75)]);
}
