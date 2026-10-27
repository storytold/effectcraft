use super::*;
use effectcraft_engine::geom::{Mat4, vec3};

/// #1544: Shift-clicking with the Pen keeps the new segment horizontal, vertical or diagonal.
#[test]
fn pen_shift_click_keeps_the_segment_at_45_degrees() {
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
    let from = [100.0, 100.0];
    assert!(near(constrain_45(from, [104.0, 180.0]), [100.0, 180.0]), "straight down");
    assert!(near(constrain_45(from, [30.0, 97.0]), [30.0, 100.0]), "left");
    assert!(near(constrain_45(from, [150.0, 46.0]), [152.0, 48.0]), "up and right, on the diagonal");
    assert!(near(constrain_45(from, from), from), "on the vertex");
    assert!(constrain_45(from, [f64::NAN, 3.0])[0].is_nan(), "a non-finite point stays as it is");
}

/// The Rotation tool counts past a full turn in both directions, as in After Effects: the
/// pointer going round the anchor 1.5 turns clockwise gives 540°, then back 2 turns gives
/// −180°, with no jump where atan2 wraps from 180° to −180°.
#[test]
fn rotate_tool_accumulates_revolutions() {
    // Screen y points down, so a growing atan2 angle is clockwise on screen.
    let drag = |start: f64, steps: &[f64]| {
        let (mut last, mut turned, mut out) = (start, 0.0, vec![]);
        for &a in steps {
            // atan2's range: (−180°, 180°].
            let a = (a + 180.0).rem_euclid(360.0) - 180.0;
            turned += turn_delta(last, a);
            last = a;
            out.push(turned);
        }
        (last, turned, out)
    };
    let cw: Vec<f64> = (1..=54).map(|i| i as f64 * 10.0).collect();
    let (last, turned, seq) = drag(0.0, &cw);
    assert!((turned - 540.0).abs() < 1e-9, "{turned}");
    assert!(seq.windows(2).all(|w| (w[1] - w[0] - 10.0).abs() < 1e-9), "no jump: {seq:?}");
    assert!((rotate_value(0.0, turned, false) - 540.0).abs() < 1e-9);
    let ccw: Vec<f64> = (1..=72).map(|i| 540.0 - i as f64 * 10.0).collect();
    let (_, back, seq) = drag(last, &ccw);
    assert!(seq.windows(2).all(|w| (w[1] - w[0] + 10.0).abs() < 1e-9), "no jump: {seq:?}");
    let total = turned + back;
    assert!((total + 180.0).abs() < 1e-9, "{total}");
    assert!((rotate_value(30.0, total, false) + 150.0).abs() < 1e-9);
    // Shift snaps the accumulated value, not a wrapped one.
    assert_eq!(rotate_value(0.0, 400.0, true), 405.0);
    assert_eq!(rotate_value(0.0, -400.0, true), -405.0);
    // Across the seam: 170° → −170° is +20°, and back is −20°; exactly half a turn is +180°.
    assert!((turn_delta(170.0, -170.0) - 20.0).abs() < 1e-9);
    assert!((turn_delta(-170.0, 170.0) + 20.0).abs() < 1e-9);
    assert_eq!(turn_delta(0.0, 180.0), 180.0);
    assert_eq!(turn_delta(0.0, -180.0), 180.0);
    // Never crash or poison the value on non-finite input.
    assert_eq!(turn_delta(f64::NAN, 10.0), 0.0);
    assert_eq!(turn_delta(0.0, f64::INFINITY), 0.0);
    assert_eq!(rotate_value(15.0, f64::NAN, false), 15.0);
}

/// #1042: a path vertex lying under, or just beside, a bounding-box handle is grabbed
/// instead of the handle; away from every handle the reach is unchanged.
#[test]
fn vertex_reaches_as_far_as_a_box_handle_over_it() {
    let handle = pos2(100.0, 100.0);
    let vertex = pos2(100.0, 100.0);
    // The press lands 8 points off the vertex, still on the handle's square.
    let press = pos2(108.0, 100.0);
    let pick = |p: Pos2, handles: &[Pos2]| vertex.distance(p) < vertex_grab_radius(p, handles.iter().copied());
    assert!(handle.distance(press) < HANDLE + 2.0, "the handle would take this press");
    assert!(pick(press, &[handle]), "the vertex wins over the handle");
    assert!(!pick(press, &[]), "no handle there: the usual reach");
    assert!(pick(pos2(105.0, 100.0), &[]));
    assert!(!pick(pos2(120.0, 100.0), &[handle]), "past the handle: nothing");
}

/// #882: the frame's corner sits on a physical pixel at any display scaling, so 100 % maps
/// comp pixels 1:1 onto screen pixels.
#[test]
fn frame_origin_snaps_to_physical_pixels() {
    for ppp in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
        for p in [pos2(-201.12, 72.4), pos2(689.8, 239.75), pos2(0.3, -0.37)] {
            let s = snap_to_pixels(p, ppp);
            for v in [s.x * ppp, s.y * ppp] {
                assert!((v - v.round()).abs() < 1e-3, "ppp {ppp}: {p:?} → {s:?}");
            }
            assert!((s - p).length() <= 0.71 / ppp, "moved by at most half a pixel: {p:?} → {s:?}");
        }
    }
    assert_eq!(snap_to_pixels(pos2(1.3, 2.0), 0.0), pos2(1.3, 2.0));
}

/// #882: a magnified frame shows its pixels as blocks in both Zoom Quality modes; More
/// Accurate only smooths when the frame is shown smaller.
#[test]
fn magnified_frames_are_never_smoothed() {
    use eframe::wgpu::FilterMode;
    for smooth in [true, false] {
        assert_eq!(zoom_texture_options(smooth).magnification, egui::TextureFilter::Nearest);
        assert_eq!(zoom_sampler(smooth).mag_filter, FilterMode::Nearest);
    }
    assert_eq!(zoom_texture_options(true).minification, egui::TextureFilter::Linear);
    assert_eq!(zoom_sampler(true).min_filter, FilterMode::Linear);
}

/// A perspective-like homography: w = 100 + y (the plane crosses the camera at y = −100).
fn tilted() -> Mat3 {
    Mat3([[100.0, 0.0, 0.0], [0.0, 100.0, 0.0], [0.0, 1.0, 100.0]])
}

/// #572: a box reaching behind the camera is clipped at the near plane instead of flipping
/// through infinity into long lines.
#[test]
fn outlines_clip_at_the_near_plane() {
    let m = tilted();
    let front = project_clipped(&m, &box_corners([-50.0, -50.0, 50.0, 50.0]));
    assert_eq!(front.len(), 4);
    let crossing = project_clipped(&m, &box_corners([-50.0, -150.0, 50.0, 50.0]));
    assert_eq!(crossing.len(), 4, "{crossing:?}");
    // Nothing flipped through infinity: a corner behind the camera (y = −150, w = −50) would
    // land at y = +300 if it were projected as is; clipped at the near plane it runs off far
    // above (negative y) instead, and the front corners stay where they were.
    assert!(crossing.iter().all(|p| p[0].is_finite() && p[1].is_finite() && p[1] < 34.0), "{crossing:?}");
    assert!(project_clipped(&m, &box_corners([-50.0, -400.0, 50.0, -200.0])).is_empty(), "wholly behind");
}

#[test]
fn axis_drag_follows_the_arrow_and_ignores_drag_across_it() {
    let (ax, ay) = axis_drag([0.0, 0.0, 60.0], [24.0, -24.0]);
    let at = |d: [f64; 2]| [0, 1, 2].map(|i| ax[i] * d[0] + ay[i] * d[1]);
    assert_eq!(at([24.0, -24.0]), [0.0, 0.0, 60.0]);
    assert_eq!(at([-12.0, 12.0]), [0.0, 0.0, -30.0]);
    assert_eq!(at([10.0, 10.0]), [0.0, 0.0, 0.0]);
}

#[test]
fn gizmo_segments_clip_at_the_near_plane() {
    // w = 100 + z.
    let pc = Mat4([[100.0, 0.0, 0.0, 0.0], [0.0, 100.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 1.0, 100.0]]);
    let s = project_segment(&pc, vec3(10.0, 0.0, 0.0), vec3(10.0, 0.0, -300.0)).unwrap();
    assert_eq!(s[0], [10.0, 0.0]);
    assert!(s[1][0] > 10.0 && s[1][0].is_finite(), "toward the camera it grows, never flips: {s:?}");
    assert!(project_segment(&pc, vec3(0.0, 0.0, -200.0), vec3(0.0, 0.0, -300.0)).is_none());
}
