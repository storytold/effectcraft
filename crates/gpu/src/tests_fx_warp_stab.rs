//! GPU warp-stabilizer vs CPU warp-stabilizer parity tests.
//!
//! Runs `ec.distort.warpstabilizer` through both the GPU and CPU paths for each Method ×
//! Framing combination, with a non-trivial synthetic analysis (rotation + scale + translation).
//! Includes a 4K test case. Tolerance: 1/255 per channel (same as the existing warp tests).

use effectcraft_keyframe::Value;
use effectcraft_track::stabilize::{FrameMotion, WarpAnalysis};

use crate::tests::{effect_case, n};

fn s(v: &str) -> Value {
    Value::Str(v.to_owned())
}
fn e(v: u32) -> Value {
    Value::Enum(v)
}

/// Build a WarpAnalysis with non-trivial per-frame motion: rotation + scale + translation.
fn synthetic_analysis(n: usize, w: f64, h: f64) -> WarpAnalysis {
    let mut a =
        WarpAnalysis { version: 1, start: 0.0, frame_duration: 1.0 / 25.0, size: [w, h], detailed: false, frames: Vec::with_capacity(n), tracks: vec![] };
    for k in 0..n {
        let angle = (k as f64 * 0.02).sin() * 5.0_f64.to_radians();
        let scale = 1.0 + (k as f64 * 0.03).cos() * 0.02;
        let tx = (k as f64 * 1.3).sin() * 3.0;
        let ty = (k as f64 * 0.9).cos() * 2.5;
        let (ca, sa) = (angle.cos(), angle.sin());
        let fm = FrameMotion {
            t: [tx, ty],
            s: [ca * scale, sa * scale, -sa * scale, ca * scale],
            h: [ca * scale, -sa * scale, tx, sa * scale, ca * scale, ty, 0.00001 * k as f64, 0.00001 * k as f64],
            points: (0..4)
                .map(|j| {
                    let px = w * 0.25 + j as f64 * w * 0.25;
                    let py = h * 0.5 + ((j as f64 * 0.3) + k as f64 * 0.1).sin() * h * 0.1;
                    [px as f32, py as f32]
                })
                .collect(),
            ..Default::default()
        };
        a.frames.push(fm);
    }
    a
}

fn analysis_json(a: &WarpAnalysis) -> String {
    a.to_json()
}

// ── GPU parity tests ─────────────────────────────────────────────────────────

#[test]
fn warpstab_position_stabilize_only() {
    let a = analysis_json(&synthetic_analysis(10, 70.0, 44.0));
    effect_case("ec.distort.warpstabilizer", &[("analysis", s(&a)), ("borders/framing", e(0)), ("stabilization/method", e(0))]);
}

#[test]
fn warpstab_position_stabilize_crop() {
    let a = analysis_json(&synthetic_analysis(10, 70.0, 44.0));
    effect_case("ec.distort.warpstabilizer", &[("analysis", s(&a)), ("borders/framing", e(1)), ("stabilization/method", e(0))]);
}

#[test]
fn warpstab_position_stabilize_crop_autoscale() {
    let a = analysis_json(&synthetic_analysis(10, 70.0, 44.0));
    effect_case(
        "ec.distort.warpstabilizer",
        &[("analysis", s(&a)), ("borders/framing", e(2)), ("stabilization/method", e(0)), ("borders/autoScale/maximumScale", n(150.0))],
    );
}

#[test]
fn warpstab_similarity() {
    let a = analysis_json(&synthetic_analysis(10, 70.0, 44.0));
    effect_case("ec.distort.warpstabilizer", &[("analysis", s(&a)), ("borders/framing", e(2)), ("stabilization/method", e(1))]);
}

#[test]
fn warpstab_perspective() {
    let a = analysis_json(&synthetic_analysis(10, 70.0, 44.0));
    effect_case("ec.distort.warpstabilizer", &[("analysis", s(&a)), ("borders/framing", e(2)), ("stabilization/method", e(2))]);
}

#[test]
fn warpstab_perspective_stabilize_only() {
    let a = analysis_json(&synthetic_analysis(10, 70.0, 44.0));
    effect_case("ec.distort.warpstabilizer", &[("analysis", s(&a)), ("borders/framing", e(0)), ("stabilization/method", e(2))]);
}

#[test]
fn warpstab_no_motion() {
    // No Motion result: locks the scene exactly.
    let a = analysis_json(&synthetic_analysis(5, 70.0, 44.0));
    effect_case("ec.distort.warpstabilizer", &[("analysis", s(&a)), ("borders/framing", e(0)), ("stabilization/method", e(0)), ("stabilization/result", e(1))]);
}

#[test]
fn warpstab_additional_scale() {
    let a = analysis_json(&synthetic_analysis(10, 70.0, 44.0));
    effect_case(
        "ec.distort.warpstabilizer",
        &[
            ("analysis", s(&a)),
            ("borders/framing", e(2)),
            ("stabilization/method", e(1)),
            ("borders/additionalScale", n(130.0)),
            ("borders/autoScale/actionSafeMargin", n(5.0)),
        ],
    );
}

#[test]
fn warpstab_smoothness_variations() {
    let a = analysis_json(&synthetic_analysis(10, 70.0, 44.0));
    effect_case(
        "ec.distort.warpstabilizer",
        &[("analysis", s(&a)), ("borders/framing", e(2)), ("stabilization/method", e(1)), ("stabilization/smoothness", n(5.0))],
    );
    effect_case(
        "ec.distort.warpstabilizer",
        &[("analysis", s(&a)), ("borders/framing", e(2)), ("stabilization/method", e(1)), ("stabilization/smoothness", n(200.0))],
    );
}

#[test]
fn warpstab_preserve_scale() {
    let a = analysis_json(&synthetic_analysis(10, 70.0, 44.0));
    effect_case(
        "ec.distort.warpstabilizer",
        &[("analysis", s(&a)), ("borders/framing", e(2)), ("stabilization/method", e(1)), ("stabilization/preserveScale", Value::Bool(true))],
    );
}

// The 4K test case: a large frame with a non-trivial transform.
#[test]
fn warpstab_4k() {
    let a = analysis_json(&synthetic_analysis(10, 1920.0, 1080.0));
    effect_case(
        "ec.distort.warpstabilizer",
        &[
            ("analysis", s(&a)),
            ("borders/framing", e(2)),
            ("stabilization/method", e(1)),
            ("borders/autoScale/maximumScale", n(120.0)),
            ("borders/additionalScale", n(110.0)),
        ],
    );
}
