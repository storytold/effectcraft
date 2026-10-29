use super::*;
use kurbo::{Circle, Rect, Shape};

fn len(p: &[BezPath]) -> f64 {
    p.iter().map(length).sum()
}

fn sq(x: f64, y: f64, s: f64) -> BezPath {
    Rect::new(x, y, x + s, y + s).to_path(0.1)
}

fn circle(x: f64, y: f64, r: f64) -> BezPath {
    Circle::new((x, y), r).to_path(1e-3)
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn on_curve(p: &BezPath) -> Vec<Point> {
    p.elements().iter().filter_map(|e| e.end_point()).collect()
}

// ---------------------------------------------------------------- trim

#[test]
fn trim_half_square() {
    let s = sq(0.0, 0.0, 100.0);
    let t = trim(std::slice::from_ref(&s), 0.0, 50.0, 0.0);
    assert!(close(len(&t), 200.0, 0.5), "{}", len(&t));
    let t2 = trim(&[s], 0.0, 50.0, 270.0);
    assert!(close(len(&t2), 200.0, 0.5));
}

#[test]
fn trim_empty_keeps_slot() {
    let t = trim(&[sq(0.0, 0.0, 10.0)], 30.0, 30.0, 0.0);
    assert_eq!(t.len(), 1);
    assert!(t[0].elements().is_empty());
}

#[test]
fn trim_across_start_point_is_one_piece() {
    // 75 %..125 % (via the offset) crosses the start of the closed path: one continuous subpath.
    let t = trim(&[sq(0.0, 0.0, 100.0)], 0.0, 50.0, 270.0);
    let moves = t[0].elements().iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count();
    assert_eq!(moves, 1, "{:?}", t[0]);
}

#[test]
fn trim_simultaneously_trims_each_path() {
    let paths = [sq(0.0, 0.0, 100.0), sq(200.0, 0.0, 50.0)];
    let t = trim(&paths, 0.0, 50.0, 0.0);
    assert!(close(length(&t[0]), 200.0, 0.5));
    assert!(close(length(&t[1]), 100.0, 0.5));
}

#[test]
fn trim_individually_is_sequential() {
    let paths = [sq(0.0, 0.0, 100.0), sq(200.0, 0.0, 100.0)];
    let t = trim_individually(&paths, 0.0, 50.0, 0.0);
    assert!(close(length(&t[0]), 400.0, 0.5), "{}", length(&t[0]));
    assert!(length(&t[1]) < 1e-6);
    let t = trim_individually(&paths, 25.0, 75.0, 0.0);
    assert!(close(length(&t[0]), 200.0, 0.5));
    assert!(close(length(&t[1]), 200.0, 0.5));
    // Unequal lengths: the kept length is the fraction of the total.
    let paths = [sq(0.0, 0.0, 100.0), sq(200.0, 0.0, 25.0)];
    let t = trim_individually(&paths, 10.0, 90.0, 45.0);
    assert!(close(len(&t), 0.8 * 500.0, 0.5), "{}", len(&t));
}

#[test]
fn reverse_flips_orientation() {
    let s = sq(0.0, 0.0, 10.0);
    let r = reverse(std::slice::from_ref(&s));
    assert!(close(r[0].area(), -s.area(), 1e-9));
    assert_eq!(vertex_count(&r[0]), 4);
}

// ---------------------------------------------------------------- round corners

#[test]
fn round_corners_square() {
    let r = round_corners(&[sq(0.0, 0.0, 100.0)], 10.0);
    assert_eq!(vertex_count(&r[0]), 8);
    let expect = 10000.0 - (4.0 - PI) * 100.0;
    assert!(close(r[0].area().abs(), expect, 1.0), "{}", r[0].area());
    // Matches the rectangle's own roundness.
    let rr = crate::rect([100.0, 100.0], [50.0, 50.0], 10.0);
    assert!(close(rr.area().abs(), r[0].area().abs(), 1.0));
}

#[test]
fn round_corners_clamps_to_half_segment() {
    let r = round_corners(&[sq(0.0, 0.0, 20.0)], 100.0);
    assert!(close(r[0].area().abs(), PI * 100.0, 2.0), "{}", r[0].area());
}

#[test]
fn round_corners_keeps_smooth_and_open_ends() {
    let c = circle(0.0, 0.0, 50.0);
    let r = round_corners(std::slice::from_ref(&c), 10.0);
    assert_eq!(vertex_count(&r[0]), vertex_count(&c));
    let mut open = BezPath::new();
    open.move_to((0.0, 0.0));
    open.line_to((100.0, 0.0));
    open.line_to((100.0, 100.0));
    let r = round_corners(&[open], 20.0);
    assert_eq!(vertex_count(&r[0]), 4);
    let pts = on_curve(&r[0]);
    assert_eq!(pts[0], Point::new(0.0, 0.0));
    assert!(pts.last().unwrap().distance(Point::new(100.0, 100.0)) < 1e-9);
}

#[test]
fn round_corners_triangle_arc_is_tangent() {
    let mut t = BezPath::new();
    t.move_to((0.0, 0.0));
    t.line_to((100.0, 0.0));
    t.line_to((50.0, 80.0));
    t.close_path();
    let r = round_corners(std::slice::from_ref(&t), 15.0);
    assert_eq!(vertex_count(&r[0]), 6);
    assert!(r[0].area().abs() < t.area().abs());
    assert!(r[0].area().abs() > t.area().abs() - 3.0 * 15.0 * 15.0);
}

// ---------------------------------------------------------------- offset

#[test]
fn offset_grows_square_with_miter() {
    let o = offset(&[sq(0.0, 0.0, 100.0)], 10.0, Join::Miter, 4.0, 1.0, 0.0);
    assert!(close(area(&o), 120.0 * 120.0, 1.0), "{}", area(&o));
    let b = o[0].bounding_box();
    assert!(close(b.x0, -10.0, 1e-3) && close(b.x1, 110.0, 1e-3));
}

#[test]
fn offset_round_join_adds_perimeter_times_d() {
    let o = offset(&[sq(0.0, 0.0, 100.0)], 10.0, Join::Round, 4.0, 1.0, 0.0);
    let expect = 10000.0 + 400.0 * 10.0 + PI * 100.0;
    assert!(close(area(&o), expect, 2.0), "{} vs {expect}", area(&o));
    let o = offset(&[sq(0.0, 0.0, 100.0)], 10.0, Join::Bevel, 4.0, 1.0, 0.0);
    let expect = 10000.0 + 400.0 * 10.0 + 4.0 * 50.0;
    assert!(close(area(&o), expect, 2.0), "{} vs {expect}", area(&o));
}

#[test]
fn offset_negative_shrinks() {
    let o = offset(&[sq(0.0, 0.0, 100.0)], -10.0, Join::Miter, 4.0, 1.0, 0.0);
    assert!(close(area(&o), 80.0 * 80.0, 1.0), "{}", area(&o));
    let gone = offset(&[sq(0.0, 0.0, 100.0)], -60.0, Join::Miter, 4.0, 1.0, 0.0);
    assert!(area(&gone) < 1e-6);
}

#[test]
fn offset_circle_is_a_circle() {
    let o = offset(&[circle(0.0, 0.0, 50.0)], 10.0, Join::Round, 4.0, 1.0, 0.0);
    assert!(close(area(&o), PI * 3600.0, 3600.0 * PI * 1e-3), "{}", area(&o));
    let o = offset(&[circle(0.0, 0.0, 50.0)], -20.0, Join::Round, 4.0, 1.0, 0.0);
    assert!(close(area(&o), PI * 900.0, 900.0 * PI * 1e-3), "{}", area(&o));
    // Curves stay curves (no polygon flattening blow-up).
    assert!(vertex_count(&o[0]) < 40, "{}", vertex_count(&o[0]));
}

#[test]
fn offset_is_independent_of_orientation() {
    let s = sq(0.0, 0.0, 100.0);
    let a = offset(std::slice::from_ref(&s), 5.0, Join::Miter, 4.0, 1.0, 0.0);
    let b = offset(&reverse(&[s]), 5.0, Join::Miter, 4.0, 1.0, 0.0);
    assert!(close(area(&a), area(&b), 1e-3));
}

#[test]
fn offset_copies() {
    let o = offset(&[sq(0.0, 0.0, 100.0)], 10.0, Join::Miter, 4.0, 3.0, 0.0);
    let contours = o[0].elements().iter().filter(|e| matches!(e, PathEl::MoveTo(_))).count();
    assert_eq!(contours, 3);
    assert!(close(o[0].bounding_box().width(), 160.0, 1e-3));
    let o = offset(&[sq(0.0, 0.0, 100.0)], 10.0, Join::Miter, 4.0, 1.0, 1.0);
    assert!(close(o[0].bounding_box().width(), 140.0, 1e-3));
}

#[test]
fn offset_open_path_outlines() {
    let mut l = BezPath::new();
    l.move_to((0.0, 0.0));
    l.line_to((100.0, 0.0));
    let o = offset(&[l], 5.0, Join::Miter, 4.0, 1.0, 0.0);
    assert!(close(area(&o), 1000.0, 1.0), "{}", area(&o));
}

// ---------------------------------------------------------------- pucker & bloat

#[test]
fn bloat_moves_vertices_inwards_and_bulges() {
    let s = sq(-50.0, -50.0, 100.0);
    let b = pucker_bloat(std::slice::from_ref(&s), 50.0);
    for p in on_curve(&b[0]) {
        assert!(close(p.x.abs(), 25.0, 1e-9) && close(p.y.abs(), 25.0, 1e-9), "{p:?}");
    }
    // Handles pushed outwards: the shape reaches beyond its vertices.
    let bb = b[0].bounding_box();
    assert!(bb.width() > 50.0 + 1.0, "{bb:?}");
}

#[test]
fn pucker_moves_vertices_outwards() {
    let s = sq(-50.0, -50.0, 100.0);
    let p = pucker_bloat(std::slice::from_ref(&s), -50.0);
    for q in on_curve(&p[0]) {
        assert!(close(q.x.abs(), 75.0, 1e-9) && close(q.y.abs(), 75.0, 1e-9));
    }
    assert!(area(&p) < area(&[s]) * 2.25, "segments pulled in");
    assert_eq!(pucker_bloat(&[sq(0.0, 0.0, 1.0)], 0.0)[0], sq(0.0, 0.0, 1.0));
}

// ---------------------------------------------------------------- twist

#[test]
fn twist_rotates_by_distance() {
    let mut l = BezPath::new();
    l.move_to((0.0, 0.0));
    l.line_to((100.0, 0.0));
    let t = twist(&[l], 90.0, [0.0, 0.0]);
    let pts = on_curve(&t[0]);
    assert!(pts[0].distance(Point::ZERO) < 1e-9, "centre is fixed");
    let end = *pts.last().unwrap();
    assert!(end.distance(Point::new(0.0, 100.0)) < 1e-9, "{end:?}");
    // Every on-curve point keeps its distance from the centre (it is a rotation per radius).
    for p in &pts {
        let r = p.distance(Point::ZERO);
        let ang = p.y.atan2(p.x);
        assert!(close(ang, PI / 2.0 * r / 100.0, 1e-9), "{p:?}");
    }
    assert!(pts.len() > 4, "subdivided");
}

#[test]
fn twist_of_centred_circle_keeps_it_round() {
    let c = circle(0.0, 0.0, 50.0);
    let t = twist(std::slice::from_ref(&c), 120.0, [0.0, 0.0]);
    assert!(close(area(&t), area(&[c]), 1.0));
    let id = twist(&[sq(0.0, 0.0, 5.0)], 0.0, [0.0, 0.0]);
    assert_eq!(id[0], sq(0.0, 0.0, 5.0));
}

// ---------------------------------------------------------------- zig zag

#[test]
fn zigzag_vertex_counts() {
    let z = zigzag(&[sq(0.0, 0.0, 100.0)], 5.0, 5.0, false);
    assert_eq!(vertex_count(&z[0]), 24);
    let mut l = BezPath::new();
    l.move_to((0.0, 0.0));
    l.line_to((100.0, 0.0));
    let z = zigzag(&[l], 5.0, 3.0, false);
    assert_eq!(vertex_count(&z[0]), 5);
    let pts = on_curve(&z[0]);
    for (i, p) in pts.iter().enumerate() {
        assert!(close(p.x, 25.0 * i as f64, 1e-6));
        assert!(close(p.y.abs(), 5.0, 1e-9));
        if i > 0 {
            assert!(p.y * pts[i - 1].y < 0.0, "alternates");
        }
    }
}

#[test]
fn zigzag_smooth_uses_curves() {
    let z = zigzag(&[sq(0.0, 0.0, 100.0)], 5.0, 2.0, true);
    assert!(z[0].elements().iter().any(|e| matches!(e, PathEl::CurveTo(..))));
    assert_eq!(vertex_count(&z[0]), 12);
    let corner = zigzag(&[sq(0.0, 0.0, 100.0)], 5.0, 2.0, false);
    assert!(!corner[0].elements().iter().any(|e| matches!(e, PathEl::CurveTo(..))));
}

// ---------------------------------------------------------------- wiggle

#[test]
fn wiggle_is_deterministic_and_time_varying() {
    let s = [sq(0.0, 0.0, 100.0)];
    let w = WiggleParams::default();
    let a = wiggle(&s, &w, 0.5);
    assert_eq!(a, wiggle(&s, &w, 0.5));
    assert_ne!(a, wiggle(&s, &w, 0.75));
    assert_ne!(a, wiggle(&s, &WiggleParams { seed: 3.0, ..w }, 0.5));
    // Phase of 360° = one wiggle period at speed 1: same as advancing time by one second.
    let w1 = WiggleParams { speed: 1.0, ..w };
    assert_eq!(wiggle(&s, &WiggleParams { phase_deg: 360.0, ..w1 }, 0.25), wiggle(&s, &w1, 1.25));
    assert_eq!(vertex_count(&a[0]), 4 * 11);
}

#[test]
fn wiggle_displacement_is_bounded() {
    let mut l = BezPath::new();
    l.move_to((0.0, 0.0));
    l.line_to((1000.0, 0.0));
    let w = WiggleParams { size: 7.0, detail: 50.0, smooth: false, ..Default::default() };
    for t in [0.0, 0.3, 1.7, 9.1] {
        let r = wiggle(std::slice::from_ref(&l), &w, t);
        let pts = on_curve(&r[0]);
        assert_eq!(pts.len(), 52);
        assert!(pts.iter().all(|p| p.y.abs() <= 7.0 * 1.0001));
        assert!(pts.iter().any(|p| p.y.abs() > 0.1));
    }
}

#[test]
fn wiggle_full_correlation_translates() {
    let s = [sq(0.0, 0.0, 100.0)];
    let w = WiggleParams { correlation: 100.0, smooth: false, ..Default::default() };
    let r = wiggle(&s, &w, 0.4);
    assert!(close(area(&r), 10000.0, 1e-6));
    let zero = wiggle(&s, &WiggleParams { size: 0.0, ..w }, 1.0);
    assert_eq!(zero[0], s[0]);
}

/// Spatial Phase moves the pattern of displacements along the path: 360° is once around, and a
/// whole number of points' worth gives each point the displacement of the point that far ahead.
#[test]
fn wiggle_spatial_phase_moves_the_pattern_along_the_path() {
    let s = [sq(0.0, 0.0, 100.0)];
    let w = WiggleParams { detail: 3.0, correlation: 0.0, smooth: false, ..Default::default() };
    // Displacements are proportional to Size: twice the size less once the size.
    let disp = |w: WiggleParams| -> Vec<Vec2> {
        let a = on_curve(&wiggle(&s, &w, 0.4)[0]);
        let b = on_curve(&wiggle(&s, &WiggleParams { size: w.size * 2.0, ..w }, 0.4)[0]);
        a.iter().zip(&b).map(|(a, b)| *b - *a).take(16).collect()
    };
    let d0 = disp(w);
    let n = d0.len();
    assert_eq!(n, 16);
    for k in [1, 5] {
        let dk = disp(WiggleParams { spatial_phase_deg: 360.0 * k as f64 / n as f64, ..w });
        for j in 0..n {
            let e = d0[(j + k) % n];
            assert!(close(dk[j].x, e.x, 1e-9) && close(dk[j].y, e.y, 1e-9), "{k} ahead, point {j}: {:?} vs {e:?}", dk[j]);
        }
    }
    // Whole turns either way leave the path as it is.
    let base = wiggle(&s, &w, 0.4);
    assert_eq!(wiggle(&s, &WiggleParams { spatial_phase_deg: 360.0, ..w }, 0.4), base);
    assert_eq!(wiggle(&s, &WiggleParams { spatial_phase_deg: -720.0, ..w }, 0.4), base);
    // Half a point ahead, the displacements change and stay within Size.
    let half = disp(WiggleParams { spatial_phase_deg: 180.0 / n as f64, ..w });
    assert!(half.iter().zip(&d0).any(|(h, d)| (*h - *d).hypot() > 1e-3));
    assert!(half.iter().all(|h| h.x.abs() <= 10.0 + 1e-9 && h.y.abs() <= 10.0 + 1e-9));
}

// ---------------------------------------------------------------- merge / booleans

fn inclusion_exclusion(a: BezPath, b: BezPath) {
    let aa = area(std::slice::from_ref(&a));
    let ab = area(std::slice::from_ref(&b));
    let pair = [a, b];
    let u = area(&[merge(&pair, MergeMode::Add)]);
    let i = area(&[merge(&pair, MergeMode::Intersect)]);
    let s = area(&[merge(&pair, MergeMode::Subtract)]);
    let x = area(&[merge(&pair, MergeMode::Exclude)]);
    let tol = 1e-3 * (aa + ab);
    assert!(i > 1.0, "overlapping");
    assert!(close(u, aa + ab - i, tol), "union {u} vs {aa}+{ab}-{i}");
    assert!(close(s, aa - i, tol), "subtract {s}");
    assert!(close(x, u - i, tol), "exclude {x}");
}

#[test]
fn merge_rects_inclusion_exclusion() {
    inclusion_exclusion(sq(0.0, 0.0, 100.0), sq(50.0, 30.0, 100.0));
    let i = merge(&[sq(0.0, 0.0, 100.0), sq(50.0, 30.0, 100.0)], MergeMode::Intersect);
    assert!(close(area(&[i]), 50.0 * 70.0, 1e-6));
}

#[test]
fn merge_circles_inclusion_exclusion() {
    inclusion_exclusion(circle(0.0, 0.0, 50.0), circle(40.0, 10.0, 35.0));
    // Curves are preserved: the union of two circles has few vertices.
    let u = merge(&[circle(0.0, 0.0, 50.0), circle(40.0, 10.0, 35.0)], MergeMode::Add);
    assert!(vertex_count(&u) < 20, "{}", vertex_count(&u));
}

#[test]
fn merge_modes_edge_cases() {
    let a = sq(0.0, 0.0, 10.0);
    let b = sq(100.0, 0.0, 10.0);
    assert!(area(&[merge(&[a.clone(), b.clone()], MergeMode::Intersect)]) < 1e-9);
    assert!(close(area(&[merge(&[a.clone(), b.clone()], MergeMode::Add)]), 200.0, 1e-6));
    let m = merge(&[a.clone(), b.clone()], MergeMode::Merge);
    assert_eq!(m.elements().len(), a.elements().len() + b.elements().len());
    // Subtract with three operands: first minus the rest.
    let s = merge(&[sq(0.0, 0.0, 100.0), sq(0.0, 0.0, 50.0), sq(50.0, 50.0, 50.0)], MergeMode::Subtract);
    assert!(close(area(&[s]), 5000.0, 1e-6));
    assert!(merge(&[], MergeMode::Add).elements().is_empty());
}

#[test]
fn normalize_and_filled_area() {
    // Two same-direction overlapping squares in one path: non-zero fills the union.
    let mut p = sq(0.0, 0.0, 100.0);
    p.extend(sq(50.0, 0.0, 100.0).elements().iter().copied());
    assert!(close(crate::boolean::filled_area(&p, FillRule::NonZero), 15000.0, 1e-6));
    assert!(close(crate::boolean::filled_area(&p, FillRule::EvenOdd), 10000.0, 1e-6));
    let n = crate::boolean::normalize(&p, FillRule::EvenOdd);
    assert!(close(n.area().abs(), 10000.0, 1e-6));
}
