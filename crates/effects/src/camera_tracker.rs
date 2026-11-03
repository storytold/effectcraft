//! 3D Camera Tracker (Effect ▸ Perspective ▸ 3D Camera Tracker; Animation ▸ Track Camera).
//!
//! The analysis runs as a background job in the engine (`camera.analyze`): step 1 tracks
//! features through the clip ([`effectcraft_track::camtrack::TrackAnalyzer`]), step 2 solves
//! the camera ([`effectcraft_track::camtrack::solve`]). Both results are stored as JSON in the
//! instance's hidden parameters — **Tracks** (with **Tracks Key**: what the frames were made
//! from, so source / In-Out / time edits invalidate them) and **Solve** (with **Solve Key**: the
//! Shot Type, Angle of View, Solve Method and deleted points it was solved with, so changing
//! those re-solves without re-tracking). **Method Used** and **Average Error** report the solve.
//!
//! **Solve Lens Distortion** (Advanced; also on with Detailed Analysis) solves radial lens
//! distortion `k1, k2` jointly with the camera; **Undistort Footage** then renders the input
//! undistorted (the pinhole image the solved camera sees) so 3D layers line up with it.
//!
//! The effect passes its input through (undistorted when asked); with **Render Track Points**
//! on it draws the solved points (as the viewer shows them) into the frame. The viewer overlay, target and the Create
//! commands live in the engine and the UI.

use std::sync::Arc;

use crate::analysis_cache::AnalysisCache;

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use effectcraft_track::camtrack::{CameraSolve, CameraTracks, ShotType, SolveMethod, SolveSettings, linalg, point_color};
use rayon::prelude::*;

use crate::warp_stab::param;
use crate::{Buf, EffectCtx, EffectSpec, Params, num, p, popup, slider};

/// The effect's id.
pub const ID: &str = "ec.perspective.cameratracker";

/// Hidden parameters (see the module docs).
pub const TRACKS: &str = "tracks";
pub const TRACKS_KEY: &str = "tracksKey";
pub const SOLVE: &str = "solve";
pub const SOLVE_KEY: &str = "solveKey";
/// Deleted track point ids (JSON array).
pub const DELETED: &str = "deleted";
/// The "3D Tracker Camera" layer the instance created (layer id, 0 = none).
pub const CAMERA: &str = "camera";
pub const METHOD_USED: &str = "advanced/methodUsed";
pub const AVERAGE_ERROR: &str = "advanced/averageError";

pub const SHOT_TYPES: [&str; 3] = ["Fixed Angle of View", "Variable Zoom", "Specify Angle of View"];
pub const SHOW_POINTS: [&str; 2] = ["2D Source", "3D Solved"];
pub const SOLVE_METHODS: [&str; 4] = ["Auto Detect", "Typical", "Mostly Flat Scene", "Tripod Pan"];

pub fn specs() -> Vec<EffectSpec> {
    vec![EffectSpec {
        id: ID,
        name: "3D Camera Tracker",
        category: "Perspective",
        params: vec![
            p("shotType", "Shot Type", Value::Enum(0), popup(&SHOT_TYPES)),
            p("horizontalAngleOfView", "Horizontal Angle of View", num(40.0), slider(1.0, 170.0, 10.0, 120.0, 1)),
            p("showTrackPoints", "Show Track Points", Value::Enum(1), popup(&SHOW_POINTS)),
            p("renderTrackPoints", "Render Track Points", Value::Bool(false), ParamUi::Checkbox),
            p("trackPointSize", "Track Point Size", num(100.0), slider(0.0, 1000.0, 0.0, 300.0, 0)),
            p("targetSize", "Target Size", num(100.0), slider(1.0, 1000.0, 10.0, 300.0, 0)),
            p("advanced/solveMethod", "Solve Method", Value::Enum(0), popup(&SOLVE_METHODS)),
            p(METHOD_USED, "Method Used", Value::Str(String::new()), ParamUi::Hidden),
            p(AVERAGE_ERROR, "Average Error", num(0.0), ParamUi::Hidden),
            p("advanced/detailedAnalysis", "Detailed Analysis", Value::Bool(false), ParamUi::Checkbox),
            p("advanced/lensDistortion", "Solve Lens Distortion", Value::Bool(false), ParamUi::Checkbox),
            p("advanced/undistort", "Undistort Footage", Value::Bool(false), ParamUi::Checkbox),
            p("advanced/autoDeletePoints", "Auto-delete Points Across Time", Value::Bool(true), ParamUi::Checkbox),
            p("advanced/hideWarningBanner", "Hide Warning Banner", Value::Bool(false), ParamUi::Checkbox),
            p(TRACKS, "Tracks", Value::Str(String::new()), ParamUi::Hidden),
            p(TRACKS_KEY, "Tracks Key", Value::Str(String::new()), ParamUi::Hidden),
            p(SOLVE, "Solve", Value::Str(String::new()), ParamUi::Hidden),
            p(SOLVE_KEY, "Solve Key", Value::Str(String::new()), ParamUi::Hidden),
            p(DELETED, "Deleted Points", Value::Str(String::new()), ParamUi::Hidden),
            p(CAMERA, "Camera Layer", num(0.0), ParamUi::Hidden),
        ],
        render,
        gpu: false,
        float: true,
    }]
}

fn f(params: &Params, id: &str) -> f64 {
    param(params, id).map(Value::as_f64).unwrap_or(0.0)
}
fn e(params: &Params, id: &str) -> u32 {
    param(params, id).map(Value::as_enum).unwrap_or(0)
}
fn b(params: &Params, id: &str) -> bool {
    param(params, id).map(Value::as_bool).unwrap_or(false)
}
fn s<'a>(params: &'a Params, id: &str) -> &'a str {
    match param(params, id) {
        Some(Value::Str(s)) => s,
        _ => "",
    }
}

/// Deleted point ids of an instance (stored as a JSON array of numbers).
pub fn deleted(params: &Params) -> Vec<u32> {
    s(params, DELETED).trim().trim_start_matches('[').trim_end_matches(']').split(',').filter_map(|v| v.trim().parse().ok()).collect()
}

/// The solve settings of an instance.
pub fn settings(params: &Params) -> SolveSettings {
    let mut del = deleted(params);
    del.sort_unstable();
    del.dedup();
    SolveSettings {
        shot: ShotType::ALL.get(e(params, "shotType") as usize).copied().unwrap_or_default(),
        hfov: f(params, "horizontalAngleOfView"),
        method: SolveMethod::ALL.get(e(params, "advanced/solveMethod") as usize).copied().unwrap_or_default(),
        deleted: del,
        lens_distortion: b(params, "advanced/lensDistortion") || b(params, "advanced/detailedAnalysis"),
    }
}

/// What a solve depends on besides the tracks.
pub fn solve_key(st: &SolveSettings) -> String {
    let hfov = if st.shot == ShotType::SpecifyAngle { format!("{:.4}", st.hfov) } else { String::new() };
    let lens = if st.lens_distortion { "|lens" } else { "" };
    format!("{:?}|{hfov}|{:?}|{:?}{lens}", st.shot, st.method, st.deleted)
}

/// The stored feature tracks of an instance (`None` when not analysed).
pub fn tracks(params: &Params) -> Option<Arc<CameraTracks>> {
    static C: AnalysisCache<CameraTracks> = AnalysisCache::new();
    let j = s(params, TRACKS);
    if j.is_empty() {
        return None;
    }
    C.get(j, CameraTracks::from_json).map(|(_, a)| a).filter(|t| !t.is_empty())
}

/// The stored camera solve of an instance (`None` when not solved).
pub fn solve(params: &Params) -> Option<Arc<CameraSolve>> {
    static C: AnalysisCache<CameraSolve> = AnalysisCache::new();
    let j = s(params, SOLVE);
    if j.is_empty() {
        return None;
    }
    C.get(j, CameraSolve::from_json).map(|(_, a)| a).filter(|t| !t.is_empty())
}

/// Screen size (layer pixels) of a target drawn for a point at depth `z`, relative to the
/// median depth of the frame's points.
pub fn target_radius(size_pct: f64, z: f64, median_z: f64) -> f64 {
    (6.0 * size_pct / 100.0 * (median_z / z.max(1e-9))).clamp(1.5, 40.0)
}

/// The solved points visible on frame `k`: (id, layer pixel position in the footage, camera
/// depth).
pub fn projected(solve: &CameraSolve, k: usize) -> Vec<(u32, [f64; 2], f64)> {
    projected_with(solve, k, false)
}

/// [`projected`] into the footage, or (`undistorted`) into the undistorted image.
pub fn projected_with(solve: &CameraSolve, k: usize, undistorted: bool) -> Vec<(u32, [f64; 2], f64)> {
    let Some(cam) = solve.frames.get(k) else { return vec![] };
    solve
        .visible(k)
        .filter_map(|p| {
            let c = cam.to_cam(p.pos);
            solve.project(k, p.pos, undistorted).map(|q| (p.id, q, c[2]))
        })
        .collect()
}

/// Whether the instance renders its input undistorted (Undistort Footage with a solved
/// distortion).
pub fn undistorts(params: &Params, solve: &CameraSolve) -> bool {
    b(params, "advanced/undistort") && solve.distortion.is_some_and(|d| !d.is_none())
}

/// The input resampled through the solved distortion: every output pixel shows the footage
/// at its distorted position (transparent where that falls outside the layer).
fn undistort(buf: &Buf, sol: &CameraSolve) -> Buf {
    let sc = if buf.scale > 0.0 { buf.scale } else { 1.0 };
    let mut out = Image::new(buf.img.width, buf.img.height);
    let size = sol.size;
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let p = [(x as f64 + 0.5 - buf.offset[0]) / sc, (y as f64 + 0.5 - buf.offset[1]) / sc];
            let q = sol.to_image(p);
            if q[0] < 0.0 || q[1] < 0.0 || q[0] > size[0] || q[1] > size[1] {
                continue;
            }
            *px = buf.img.sample_bilinear_clamped(q[0] * sc + buf.offset[0], q[1] * sc + buf.offset[1]);
        }
    });
    Buf { img: out, offset: buf.offset, scale: buf.scale }
}

/// Median of the depths.
pub fn median_depth(v: &[(u32, [f64; 2], f64)]) -> f64 {
    let mut z: Vec<f64> = v.iter().map(|x| x.2).collect();
    if z.is_empty() {
        return 1.0;
    }
    z.sort_by(f64::total_cmp);
    z[z.len() / 2]
}

fn render(ctx: &EffectCtx, mut buf: Buf) -> Buf {
    let points = b(ctx.params, "renderTrackPoints");
    if !points && !b(ctx.params, "advanced/undistort") {
        return buf;
    }
    let Some(sol) = solve(ctx.params) else { return buf };
    if (sol.size[0] - ctx.layer_size[0]).abs() > 0.5 || (sol.size[1] - ctx.layer_size[1]).abs() > 0.5 {
        return buf;
    }
    let und = undistorts(ctx.params, &sol);
    if und {
        buf = undistort(&buf, &sol);
    }
    if !points {
        return buf;
    }
    let Some(k) = sol.frame_at(ctx.time) else { return buf };
    let pts = projected_with(&sol, k, und);
    let med = median_depth(&pts);
    let size = f(ctx.params, "trackPointSize");
    let sc = if buf.scale > 0.0 { buf.scale } else { 1.0 };
    for (id, q, z) in pts {
        let c = point_color(id);
        let r = target_radius(size, z, med) * sc;
        let at = [q[0] * sc + buf.offset[0], q[1] * sc + buf.offset[1]];
        ring(&mut buf.img, at, r, (r * 0.25).max(1.0), [c[0], c[1], c[2], 1.0]);
        disc(&mut buf.img, at, (r * 0.2).max(0.8), [c[0], c[1], c[2], 1.0]);
    }
    buf
}

fn lerp_px(a: Px, b: Px, t: f32) -> Px {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

fn cover(img: &mut Image, c: [f64; 2], r_out: f64, cov: impl Fn(f64) -> f64, col: Px) {
    let (x0, x1) = ((c[0] - r_out - 1.0).floor().max(0.0) as i64, (c[0] + r_out + 1.0).ceil() as i64);
    let (y0, y1) = ((c[1] - r_out - 1.0).floor().max(0.0) as i64, (c[1] + r_out + 1.0).ceil() as i64);
    for y in y0..y1.min(img.height as i64) {
        for x in x0..x1.min(img.width as i64) {
            let d = (x as f64 + 0.5 - c[0]).hypot(y as f64 + 0.5 - c[1]);
            let a = cov(d).clamp(0.0, 1.0) as f32;
            if a > 0.0 {
                let i = img.idx(x as u32, y as u32);
                img.data[i] = lerp_px(img.data[i], col, a);
            }
        }
    }
}

fn disc(img: &mut Image, c: [f64; 2], r: f64, col: Px) {
    cover(img, c, r, |d| r + 0.5 - d, col);
}

fn ring(img: &mut Image, c: [f64; 2], r: f64, w: f64, col: Px) {
    cover(img, c, r + w, |d| (w * 0.5 + 0.5 - (d - r).abs()).min(1.0), col);
}

/// Points of a target disc (canonical coordinates) for drawing: `n` points on the circle of
/// radius `size / 2` in the plane through `center` with normal `normal`.
pub fn target_circle(center: linalg::V3, normal: linalg::V3, radius: f64, n: usize) -> Vec<linalg::V3> {
    let a = if normal[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let u = linalg::normalize(linalg::cross(normal, a));
    let v = linalg::cross(normal, u);
    (0..n)
        .map(|i| {
            let t = i as f64 / n as f64 * std::f64::consts::TAU;
            linalg::add(center, linalg::add(linalg::scale(u, radius * t.cos()), linalg::scale(v, radius * t.sin())))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EffectEnv;
    use effectcraft_track::camtrack::{SolvedFrame, SolvedPoint};

    fn params_with(solve: &CameraSolve, render: bool) -> Params {
        let mut p = Params::default();
        p.values.insert(SOLVE.into(), Value::Str(solve.to_json()));
        p.values.insert("renderTrackPoints".into(), Value::Bool(render));
        p.values.insert("trackPointSize".into(), Value::Scalar(100.0));
        p
    }

    #[test]
    fn render_track_points_draws_targets_and_passes_through_otherwise() {
        let sol = CameraSolve {
            version: 1,
            size: [64.0, 48.0],
            start: 0.0,
            frame_duration: 1.0 / 30.0,
            frames: vec![SolvedFrame { rot: [0.0; 3], center: [0.0; 3], focal: 50.0, solved: true }],
            points: vec![SolvedPoint { id: 3, pos: [0.0, 0.0, 1.0], error: 0.1, first: 0, last: 0 }],
            ..Default::default()
        };
        let img = Image::new(64, 48);
        for on in [false, true] {
            let params = params_with(&sol, on);
            let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [64.0, 48.0], seed: 0, adjustment: false, env: EffectEnv::default() };
            let out = render(&ctx, Buf { img: img.clone(), offset: [0.0; 2], scale: 1.0 });
            let px = out.img.data[out.img.idx(32, 24)];
            assert_eq!(px[3] > 0.5, on);
        }
        let st = settings(&Params::default());
        assert_eq!(st.shot, ShotType::FixedAngle);
        assert!(solve_key(&st).contains("FixedAngle"));
        assert!(!st.lens_distortion && !solve_key(&st).contains("lens"));
    }

    #[test]
    fn tracks_and_solve_follow_analysis_replacements() {
        let mut t = CameraTracks { frames: 3, start: 1.0, ..Default::default() };
        let mut sol = CameraSolve { frames: vec![SolvedFrame::default()], start: 1.0, ..Default::default() };
        let mut ps = params_with(&sol, false);
        ps.values.insert(TRACKS.into(), Value::Str(t.to_json()));
        assert_eq!(*tracks(&ps).unwrap(), t);
        assert_eq!(*solve(&ps).unwrap(), sol);
        t.start = 2.0;
        sol.start = 2.0;
        ps.values.insert(TRACKS.into(), Value::Str(t.to_json()));
        ps.values.insert(SOLVE.into(), Value::Str(sol.to_json()));
        assert_eq!(*tracks(&ps).unwrap(), t);
        assert_eq!(*solve(&ps).unwrap(), sol);
        assert_eq!(*tracks(&ps.clone()).unwrap(), t);
        ps.values.insert(TRACKS.into(), Value::Str("invalid".into()));
        ps.values.insert(SOLVE.into(), Value::Str(String::new()));
        assert!(tracks(&ps).is_none());
        assert!(solve(&ps).is_none());
    }

    #[test]
    fn undistort_footage_remaps_through_the_solved_lens() {
        use effectcraft_track::camtrack::Distortion;
        let sol = CameraSolve {
            version: 1,
            size: [64.0, 48.0],
            start: 0.0,
            frame_duration: 1.0 / 30.0,
            frames: vec![SolvedFrame { rot: [0.0; 3], center: [0.0; 3], focal: 50.0, solved: true }],
            distortion: Some(Distortion { k1: -0.2, k2: 0.0, radius: 40.0 }),
            ..Default::default()
        };
        let mut params = params_with(&sol, false);
        params.values.insert("advanced/undistort".into(), Value::Bool(true));
        assert!(undistorts(&params, &sol));
        // A horizontal ramp: undistortion keeps the centre and pulls edge content outwards.
        let mut img = Image::new(64, 48);
        for (i, px) in img.data.iter_mut().enumerate() {
            let x = (i % 64) as f32 / 64.0;
            *px = [x, x, x, 1.0];
        }
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [64.0, 48.0], seed: 0, adjustment: false, env: EffectEnv::default() };
        let out = render(&ctx, Buf { img: img.clone(), offset: [0.0; 2], scale: 1.0 });
        let at = |im: &Image, x: u32, y: u32| im.data[im.idx(x, y)][0];
        assert!((at(&out.img, 32, 24) - at(&img, 32, 24)).abs() < 0.02);
        // Barrel distortion compresses the edges: the ideal pixel at x = 56 shows footage nearer the centre.
        assert!(at(&out.img, 56, 24) < at(&img, 56, 24) - 0.01, "{} vs {}", at(&out.img, 56, 24), at(&img, 56, 24));
        let sp = sol.to_image([56.0, 24.0]);
        assert!(sp[0] < 56.0 && (sol.to_ideal(sp)[0] - 56.0).abs() < 1e-6);
    }
}
