use effectcraft_geom::{Mat3, vec2};

#[test]
fn square_to_quad_degenerate_is_finite() {
    let m = Mat3::square_to_quad([vec2(0.0, 5.0), vec2(0.0, 0.0), vec2(10.0, 0.0), vec2(20.0, 0.0)]);
    assert!(m.0.iter().flatten().all(|v| v.is_finite()), "non-finite: {m:?}");
}

#[test]
fn square_to_quad_regular_is_finite() {
    let m = Mat3::square_to_quad([vec2(0.0, 0.0), vec2(10.0, 0.0), vec2(8.0, 6.0), vec2(0.0, 5.0)]);
    assert!(m.0.iter().flatten().all(|v| v.is_finite()));
}
