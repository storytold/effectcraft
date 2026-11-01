//! Validate solved geometry and its correspondence to the current layer timing.

use effectcraft_project::{Comp, Layer};
use effectcraft_time::Tick;
use effectcraft_track::camtrack::{CameraSolve, linalg};

use super::{CMD, MAX_FRAME, MAX_FRAMES, MAX_POINTS, bad, finite};
use crate::Result;

pub(super) fn timing(comp: &Comp, layer: &Layer, solve: &CameraSolve, start: i64) -> Result<(Tick, i64)> {
    if solve.frames.len() > MAX_FRAMES || solve.points.len() > MAX_POINTS {
        return Err(bad(CMD, "export is limited to 100,000 frames and 100,000 points; split the shot"));
    }
    if solve.size.iter().any(|v| !v.is_finite() || *v < 4.0 || *v > 65_536.0 || v.fract() != 0.0) {
        return Err(bad(CMD, "invalid tracking resolution (expected 4..65536 whole pixels)"));
    }
    let fps = comp.frame_rate.as_f64();
    if comp.frame_rate.num <= 0 || comp.frame_rate.den <= 0 || !fps.is_finite() || !(1.0..=1000.0).contains(&fps) {
        return Err(bad(CMD, "invalid composition frame rate (expected 1..1000 fps)"));
    }
    let end = start
        .checked_add(solve.frames.len() as i64)
        .and_then(|n| n.checked_sub(1))
        .filter(|n| *n <= MAX_FRAME)
        .ok_or_else(|| bad(CMD, "the baked animation exceeds Blender's frame range"))?;
    finite([solve.start, solve.frame_duration, solve.average_error, layer.stretch, comp.shutter_angle, comp.shutter_phase])?;
    if layer.stretch == 0.0 || comp.width == 0 || comp.height == 0 {
        return Err(bad(CMD, "invalid composition dimensions or zero layer stretch"));
    }
    let lo = comp.frame_rate.snap_nearest(layer.in_point.max(Tick::ZERO));
    let hi = layer.out_point.min(comp.duration);
    let fd = comp.frame_duration();
    if fd.0 <= 0 || lo >= hi {
        return Err(bad(CMD, "the tracked layer has no active frames"));
    }
    let expected = ((hi.0 as i128 - lo.0 as i128 + fd.0 as i128 - 1) / fd.0 as i128) as u128;
    if expected != solve.frames.len() as u128 {
        return Err(bad(CMD, "the solve no longer matches the layer's frame range; re-analyze the tracker"));
    }
    let dt = (layer.layer_time(lo + fd) - layer.layer_time(lo)).seconds();
    if (solve.start - layer.layer_time(lo).seconds()).abs() > 1e-6 || solve.frames.len() > 1 && (solve.frame_duration - dt).abs() > 1e-6 {
        return Err(bad(CMD, "the solve no longer matches the layer's timing; re-analyze the tracker"));
    }
    Ok((lo, end))
}

pub(super) fn geometry(solve: &CameraSolve, sensor: f64) -> Result<()> {
    for frame in &solve.frames {
        finite(frame.rot.into_iter().chain(frame.center).chain([frame.focal]))?;
        if !frame.solved || frame.focal <= 0.0 {
            return Err(bad(CMD, "the solve contains unsolved frames or invalid focal lengths; re-analyze the tracker"));
        }
        let mm = sensor * frame.focal / solve.size[0];
        if !(1.0..=5000.0).contains(&mm) {
            return Err(bad(CMD, "lens outside Blender's 1..5000 mm range; choose another sensorWidth"));
        }
    }
    if let Some(ground) = &solve.ground {
        finite(ground.origin.into_iter().chain(ground.normal).chain(ground.x_axis))?;
        if (linalg::norm(ground.normal) - 1.0).abs() > 1e-4
            || (linalg::norm(ground.x_axis) - 1.0).abs() > 1e-4
            || linalg::dot(ground.normal, ground.x_axis).abs() > 1e-4
        {
            return Err(bad(CMD, "invalid ground-plane basis; set the ground plane again"));
        }
    }
    if let Some(d) = &solve.distortion {
        finite([d.k1, d.k2, d.radius])?;
    }
    Ok(())
}
