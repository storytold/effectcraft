//! GPU time effects (Time Difference, Time Displacement, CC Force Motion Blur, CC Wide Time,
//! Pixel Motion Blur, Timewarp) vs the CPU effects (the oracle). They fetch other frames through
//! the effect host: a precomp whose content moves, at a nonzero time, composited at 8 and
//! 32 bpc (the direct cases have no host and pass the layer through).

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::{BitDepth, Comp, ItemKind, LayerSource, build};
use effectcraft_render::RenderOpts;
use effectcraft_time::{FrameRate, Tick};

use crate::tests::{Scene, check, compare_at, effect_case, n, opts, set, v3};

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn on() -> Value {
    Value::Bool(true)
}

/// `id` with `vals` on a moving precomp; `map` adds a hidden footage layer and sets parameter
/// `map` to it.
fn moving(id: &str, vals: &[(&str, Value)], map: Option<&str>) {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        s.push(bg);
        let mut vals = vals.to_vec();
        if let Some(param) = map {
            let mut m = s.footage(50, 36);
            m.switches.video = false;
            let mid = s.push(m);
            vals.push((param, Value::Layer(Some(mid.0))));
        }
        let inner = Comp::new(60, 40, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let inner_id = s.p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let mut a = s.footage(30, 24);
        let pos = a.props.prop_mut("transform/position").unwrap();
        pos.keys = vec![Keyframe::new(Tick::ZERO, v3(10.0, 12.0)), Keyframe::new(Tick::from_seconds_f64(1.0), v3(50.0, 30.0))];
        let rot = a.props.prop_mut("transform/rotation").unwrap();
        rot.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(90.0))];
        s.p.comp_mut(inner_id).unwrap().layers = vec![a];
        let mut l = build::layer(&mut s.p, &s.comp, "Inner", LayerSource::Comp { item: inner_id }, (60, 40), None);
        s.effect(&mut l, id, &vals);
        set(&mut l, "transform/rotation", Value::Scalar(8.0));
        s.push(l);
        let t = Tick::from_seconds_f64(0.4);
        let label = format!("{id} {vals:?} {depth:?}");
        check(&label, compare_at(&s, opts(), t), 0.0);
        check(&format!("{label} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, t), 0.0);
    }
}

#[test]
fn without_a_host() {
    for id in
        ["ec.time.timedifference", "ec.time.timedisplacement", "ec.time.ccforcemotionblur", "ec.time.ccwidetime", "ec.time.pixelmotionblur", "ec.time.timewarp"]
    {
        effect_case(id, &[]);
    }
}

#[test]
fn frame_averages() {
    moving("ec.time.ccforcemotionblur", &[], None);
    moving("ec.time.ccforcemotionblur", &[("motionBlurLevels", n(5.0)), ("shutterAngle", n(400.0))], None);
    moving("ec.time.ccwidetime", &[], None);
    moving("ec.time.ccwidetime", &[("forwardSteps", n(1.0)), ("backwardSteps", n(4.0))], None);
    moving("ec.time.pixelmotionblur", &[], None);
    moving("ec.time.pixelmotionblur", &[("shutterControl", e(1)), ("shutterAngle", n(300.0)), ("shutterSamples", n(5.0))], None);
}

#[test]
fn time_difference() {
    moving("ec.time.timedifference", &[("timeOffset", n(-0.1))], None);
    for alpha in 0..9 {
        moving(
            "ec.time.timedifference",
            &[("timeOffset", n(0.2)), ("contrast", n(40.0)), ("absoluteDifference", Value::Bool(alpha % 2 == 0)), ("alphaChannel", e(alpha))],
            None,
        );
    }
    moving("ec.time.timedifference", &[("timeOffset", n(0.1)), ("alphaChannel", e(2))], Some("targetLayer"));
}

#[test]
fn time_displacement() {
    moving("ec.time.timedisplacement", &[("maxDisplacementTime", n(0.3)), ("timeResolution", n(10.0))], None);
    moving("ec.time.timedisplacement", &[("maxDisplacementTime", n(0.5)), ("timeResolution", n(30.0))], Some("displacementMapLayer"));
    moving("ec.time.timedisplacement", &[("maxDisplacementTime", n(2.0)), ("stretchMap", Value::Bool(false))], Some("displacementMapLayer"));
}

#[test]
fn timewarp() {
    for method in 0..3 {
        moving("ec.time.timewarp", &[("method", e(method)), ("speed", n(37.0))], None);
    }
    moving("ec.time.timewarp", &[("method", e(1)), ("adjustTimeBy", e(1)), ("sourceFrame", n(7.4))], None);
    moving("ec.time.timewarp", &[("method", e(2)), ("speed", n(43.0)), ("motionBlur/enableMotionBlur", on()), ("motionBlur/shutterSamples", n(3.0))], None);
    moving(
        "ec.time.timewarp",
        &[
            ("speed", n(61.0)),
            ("tuning/filtering", e(1)),
            ("tuning/errorThreshold", n(30.0)),
            ("sourceCrops/leftCrop", n(4.0)),
            ("sourceCrops/topCrop", n(3.0)),
        ],
        None,
    );
    moving("ec.time.timewarp", &[("speed", n(55.0)), ("tuning/buildFromOneImage", on())], None);
    moving("ec.time.timewarp", &[("speed", n(47.0))], Some("warpLayer"));
    // A Matte Layer: every method and Show, the luminance channels, motion blur.
    for method in 0..3 {
        for show in 0..4 {
            moving("ec.time.timewarp", &[("method", e(method)), ("speed", n(47.0)), ("show", e(show))], Some("matteLayer"));
        }
    }
    moving("ec.time.timewarp", &[("speed", n(39.0)), ("matteChannel", e(2))], Some("matteLayer"));
    moving("ec.time.timewarp", &[("speed", n(43.0)), ("matteChannel", e(1)), ("motionBlur/enableMotionBlur", on())], Some("matteLayer"));
    moving("ec.time.timewarp", &[("method", e(1)), ("speed", n(52.0)), ("tuning/buildFromOneImage", on()), ("show", e(1))], Some("matteLayer"));
}
