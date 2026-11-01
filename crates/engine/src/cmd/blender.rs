//! Camera-solve interchange through Blender's public Python API. The generated script is
//! original MIT OR Apache-2.0 code; no Blender implementation is read or embedded.

use std::path::Path;

use effectcraft_effects::camera_tracker as ct;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, Layer, LayerId, LayerSource, Uid};
use effectcraft_render::EvalCtx;
use effectcraft_time::Tick;
use effectcraft_track::camtrack::{CameraSolve, linalg};
use serde_json::{Value, json};

mod validate;

use super::{b_p, bad, f_p, str_p};
use crate::{EngineError, Result, Session};

const CMD: &str = "camera.exportBlender";
const MAX_FRAMES: usize = 100_000;
const MAX_POINTS: usize = 100_000;
const MAX_JSON: usize = 64 * 1024 * 1024;
const MAX_FRAME: i64 = 1_048_574;

fn number(p: &Value, key: &str, default: f64, min: f64, max: f64) -> Result<f64> {
    let v = match p.get(key) {
        None => default,
        Some(_) => f_p(p, key).ok_or_else(|| bad(CMD, format!("{key}: expected a number")))?,
    };
    if !v.is_finite() || !(min..=max).contains(&v) {
        return Err(bad(CMD, format!("{key}: expected {min}..{max}")));
    }
    Ok(v)
}

fn finite(values: impl IntoIterator<Item = f64>) -> Result<()> {
    if values.into_iter().any(|v| !v.is_finite() || v.abs() > 1e12) {
        return Err(bad(CMD, "invalid or excessive numeric camera/scene data; re-analyze the tracker"));
    }
    Ok(())
}

/// Right-handed EffectCraft world (right, down, forward) → Blender (right, forward, up).
fn position(p: [f64; 3], scale: f64) -> [f64; 3] {
    [p[0] * scale, p[2] * scale, -p[1] * scale]
}

fn media_path(s: &Session, path: &str) -> Result<String> {
    if path.is_empty() || path.contains('\0') {
        return Err(bad(CMD, "the source footage has no valid file path; relink it before exporting"));
    }
    let path = Path::new(path);
    if path.is_absolute() || cfg!(target_arch = "wasm32") {
        return Ok(path.to_string_lossy().into_owned());
    }
    let cwd = std::env::current_dir().map_err(|e| bad(CMD, format!("cannot resolve footage path: {e}")))?;
    let base = s.path.as_deref().and_then(|p| Path::new(p).parent()).map(|p| cwd.join(p)).unwrap_or(cwd);
    Ok(base.join(path).to_string_lossy().into_owned())
}

pub(super) fn export(s: &mut Session, p: &Value, cid: ItemId, lid: LayerId, uid: Uid) -> Result<Value> {
    if s.running_for_script() && !s.prefs.scripting.allow_scripts_write_files {
        return Err(bad(CMD, "scripts can't export files unless Settings ▸ Scripting & Expressions ▸ Allow Scripts to Write Files and Access Network is on"));
    }
    let options = Options::parse(p)?;
    let Options { path, sensor, units, start, include_points } = options;
    if s.camera_target().is_some_and(|(c, l, u)| (c, l, u) == (cid, lid, uid)) && s.is_camera_analyzing() || s.camera_pending.contains(&(cid, lid, uid)) {
        return Err(bad(CMD, "wait for this tracker's analysis to finish before exporting"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or_else(|| bad(CMD, "tracked layer missing"))?;
    let g = layer.props.find_group(uid).ok_or_else(|| bad(CMD, "tracker missing"))?;
    for name in [ct::SOLVE, ct::TRACKS] {
        if g.prop(name).is_some_and(|p| matches!(&p.value, effectcraft_keyframe::Value::Str(v) if v.len() > MAX_JSON)) {
            return Err(bad(CMD, "tracking data exceeds the 64 MiB export limit"));
        }
    }
    let params = crate::camera_track::static_params(g);
    let solve = ct::solve(&params).ok_or_else(|| bad(CMD, "the camera is not solved yet; analyze the tracker first"))?;
    let (lo, end) = validate::timing(comp, layer, &solve, start)?;
    let Source { footage, path: source_path, fps: source_fps, count: source_count } = source(s, layer, &solve, path)?;
    validate::geometry(&solve, sensor)?;
    let camera = params
        .get(ct::CAMERA)
        .map(|v| v.as_f64())
        .filter(|v| v.is_finite() && *v > 0.0 && *v <= u64::MAX as f64)
        .and_then(|id| comp.layer(LayerId(id as u64)))
        .filter(|l| l.is_camera());
    let warnings = scene_warnings(camera, &solve, footage);
    let ctx = EvalCtx { expr: s.expr.as_deref(), ..EvalCtx::new(&s.project, cid, comp, lo) };
    let (frames, world) = bake_frames(&ctx, layer, &solve, camera, &options, footage, source_count)?;
    let mut points = Vec::new();
    for point in &solve.points {
        finite(point.pos.into_iter().chain([point.error as f64]))?;
        if point.first > point.last || point.last as usize >= solve.frames.len() {
            return Err(bad(CMD, "invalid solved point frame range; re-analyze the tracker"));
        }
        let location = position(world.apply(point.pos), units);
        finite(location)?;
        if include_points {
            points.push(json!({"id": point.id, "location": location, "error": point.error,
                "first": start.saturating_add(point.first as i64), "last": start.saturating_add(point.last as i64)}));
        }
    }
    let tracks = ct::tracks(&params);
    if tracks.as_ref().is_some_and(|t| t.tracks.len() > MAX_POINTS || t.tracks.iter().any(|t| t.pts.len() > MAX_FRAMES)) {
        return Err(bad(CMD, "2D tracks exceed export limits"));
    }
    let markers: Vec<Value> = comp
        .markers
        .iter()
        .filter_map(|m| {
            let frame = start.saturating_add(comp.frame_rate.frame_at(m.time).saturating_sub(comp.frame_rate.frame_at(lo)));
            if (start..=end).contains(&frame) { Some(json!({"name": m.comment, "frame": frame})) } else { None }
        })
        .collect();
    let payload = json!({"version": 1, "name": format!("{} — {}", s.project.item(cid).map(|i| i.name.as_str()).unwrap_or("Shot"), layer.name),
        "resolution": solve.size, "fpsNumerator": comp.frame_rate.num, "fpsDenominator": comp.frame_rate.den,
        "startFrame": start, "endFrame": end, "sensorWidth": sensor, "unitScale": units,
        "media": {"path": source_path, "kind": if footage.kind == FootageKind::Still {"FILE"} else {"MOVIE"},
            "fps": source_fps, "nativeRate": footage.native_rate, "pixelAspect": footage.pixel_aspect, "timecode": footage.start_timecode,
            "frameCount": source_count, "loopCount": footage.loop_count},
        "frames": frames, "points": points, "markers": markers, "ground": solve.ground.is_some(),
        "groundSize": world.s * units, "shutter": comp.shutter_angle / 360.0, "shutterPhase": comp.shutter_phase,
        "solve": solve, "tracks": if include_points { tracks.as_deref() } else { None },
        "warnings": warnings});
    let data = serde_json::to_string(&payload).map_err(|e| bad(CMD, format!("cannot serialize scene: {e}")))?;
    if data.len() > MAX_JSON {
        return Err(bad(CMD, "scene exceeds the 64 MiB script limit; disable includePoints or split the shot"));
    }
    let quoted = serde_json::to_string(&data).map_err(|e| bad(CMD, format!("cannot encode scene: {e}")))?;
    let script = include_str!("blender_scene.py").replace("__EFFECTCRAFT_DATA__", &quoted);
    s.services.write_file(path, script.as_bytes()).map_err(|e| bad(CMD, format!("cannot write {path}: {e}")))?;
    Ok(json!({"path": path, "bytes": script.len(), "frames": frames.len(), "points": points.len(), "warnings": warnings}))
}

struct Options<'a> {
    path: &'a str,
    sensor: f64,
    units: f64,
    start: i64,
    include_points: bool,
}

impl<'a> Options<'a> {
    fn parse(p: &'a Value) -> Result<Self> {
        let path = str_p(p, "path").filter(|p| !p.is_empty() && !p.contains('\0')).ok_or_else(|| bad(CMD, "missing `path` (.py)"))?;
        if !Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("py")) {
            return Err(bad(CMD, "the export path must end in .py"));
        }
        let sensor = number(p, "sensorWidth", 36.0, 1.0, 1000.0)?;
        let units = number(p, "unitScale", 0.01, 0.000_001, 1000.0)?;
        let start = match p.get("startFrame") {
            None => 1,
            Some(v) => v.as_i64().filter(|n| (1..=MAX_FRAME).contains(n)).ok_or_else(|| bad(CMD, "startFrame: expected a positive Blender frame number"))?,
        };
        let include_points = match p.get("includePoints") {
            Some(v) if !v.is_boolean() => return Err(bad(CMD, "includePoints: expected a boolean")),
            _ => b_p(p, "includePoints").unwrap_or(true),
        };
        Ok(Self { path, sensor, units, start, include_points })
    }
}

struct Source<'a> {
    footage: &'a Footage,
    path: String,
    fps: f64,
    count: i64,
}

fn source<'a>(s: &'a Session, layer: &Layer, solve: &CameraSolve, path: &str) -> Result<Source<'a>> {
    let footage = match layer.source {
        LayerSource::Footage { item } => s.project.item(item).and_then(|i| match &i.kind {
            ItemKind::Footage(f) => Some(f),
            _ => None,
        }),
        _ => None,
    }
    .filter(|f| f.has_video && matches!(f.kind, FootageKind::Video | FootageKind::Still));
    let footage = footage
        .ok_or_else(|| bad(CMD, "Blender background export needs a video or still-footage layer; render precomps/sequences to a video and track that video"))?;
    let source_path = media_path(s, &footage.path)?;
    if Path::new(path) == Path::new(&source_path) {
        return Err(bad(CMD, "the export path must differ from the source footage"));
    }
    if footage.width as f64 != solve.size[0] || footage.height as f64 != solve.size[1] {
        return Err(bad(CMD, "the analysis image differs from the original footage dimensions; render the tracker's input to a video and track that video"));
    }
    let source_fps = footage.frame_rate.as_f64();
    if footage.frame_rate.num <= 0 || footage.frame_rate.den <= 0 || !source_fps.is_finite() || source_fps <= 0.0 {
        return Err(bad(CMD, "invalid source footage frame rate"));
    }
    // Same frame selection as the media pool: interpreted FPS, rounded per-loop frame count,
    // clamp to the total loop range, then wrap. Native FPS does not change the frame index.
    let divisor = (effectcraft_time::TICKS_PER_SECOND as i128).saturating_mul(footage.frame_rate.den as i128);
    let source_count = (footage.duration.0 as i128).saturating_mul(footage.frame_rate.num as i128).saturating_add(divisor / 2) / divisor;
    let source_count = if footage.kind == FootageKind::Still { 1 } else { source_count };
    if !(1..=MAX_FRAME as i128).contains(&source_count) {
        return Err(bad(CMD, "invalid footage duration or footage exceeds Blender's frame range; trim/render the source video first"));
    }
    let source_count = source_count as i64;
    Ok(Source { footage, path: source_path, fps: source_fps, count: source_count })
}

fn scene_warnings(camera: Option<&Layer>, solve: &CameraSolve, footage: &Footage) -> Vec<String> {
    let mut warnings = vec![
        "Sensor width is assumed (36 mm by default); monocular tracking cannot recover physical sensor size or absolute scene scale.".to_string(),
        "The original footage is referenced, not embedded or rendered: masks and effects before the tracker are not baked. Relink the video if moving this script.".to_string(),
    ];
    if camera.is_none() {
        warnings
            .push("No linked tracker camera: focus uses the default scene depth, DOF is off; optical focus cannot be recovered from feature tracks.".into());
    } else {
        warnings.push("Focus/iris settings are sampled from the linked camera; pose and lens come from the tracker solve. Blender's physical DOF is an approximation of EffectCraft's blur model.".into());
    }
    if solve.distortion.is_some_and(|d| !d.is_none()) {
        warnings.push("Radial distortion is preserved as metadata (Brown k1/k2, half-diagonal normalization), not applied to Blender's pinhole camera. Use an undistorted plate for exact alignment.".into());
    }
    if (footage.pixel_aspect - 1.0).abs() > 1e-6 {
        warnings.push("The tracker solves square pixels: export uses square pixels; original footage pixel aspect is retained in metadata.".into());
    }
    warnings
}

/// Bake the solved camera and the linked camera's optical settings at exact composition ticks.
fn bake_frames(
    ctx0: &EvalCtx<'_>,
    layer: &Layer,
    solve: &CameraSolve,
    camera: Option<&Layer>,
    options: &Options<'_>,
    footage: &Footage,
    source_count: i64,
) -> Result<(Vec<Value>, linalg::Similarity)> {
    let comp = ctx0.comp;
    let lo = ctx0.time;
    let fd = comp.frame_duration();
    let sensor = options.sensor;
    let units = options.units;
    let start = options.start;
    let first = solve.frames.first().ok_or_else(|| bad(CMD, "the camera solve is empty"))?;
    let m = ctx0.world_matrix(layer).0;
    let layer_scale = m[0][0].hypot(m[1][0]);
    finite([layer_scale])?;
    if layer_scale <= 1e-9 {
        return Err(bad(CMD, "the tracked layer has zero scale"));
    }
    let world = solve.world([comp.width as f64, comp.height as f64], first.focal * layer_scale);
    finite([world.s * units])?;
    let mut frames = Vec::with_capacity(solve.frames.len());
    for (i, frame) in solve.frames.iter().enumerate() {
        let time = Tick(lo.0.saturating_add(fd.0.saturating_mul(i as i64)));
        let ctx = ctx0.at(time);
        let pose = linalg::mm(&world.r, &linalg::mt(&frame.r()));
        let right = position(linalg::mv(&pose, [1.0, 0.0, 0.0]), 1.0);
        let up = position(linalg::mv(&pose, [0.0, -1.0, 0.0]), 1.0);
        let back = position(linalg::mv(&pose, [0.0, 0.0, -1.0]), 1.0);
        let location = position(world.apply(frame.center), units);
        let lens = frame.focal * sensor / solve.size[0];
        let opts = camera.and_then(|l| l.props.sub("cameraOptions").map(|g| (l, g)));
        let value = |key: &str, fallback: f64| opts.map(|(l, g)| ctx.f(l, g, key, fallback)).unwrap_or(fallback);
        let focus = value("focusDistance", world.s) * units;
        let aperture = value("aperture", 0.0);
        let zoom = value("zoom", frame.focal * layer_scale);
        // Match Camera Settings' f-stop conversion: focal_for_zoom uses a 36 mm film width,
        // and the aperture UI is mm scaled by comp_width / 1920 (not a world-space diameter).
        let aperture_mm = aperture * 1920.0 / comp.width as f64;
        let fstop = if aperture_mm > 0.0 { (zoom * 36.0 / comp.width as f64) / aperture_mm } else { 2.8 };
        let rotation = value("irisRotation", 0.0).to_radians();
        let ratio = value("irisAspectRatio", 1.0);
        let shape = opts.map(|(l, g)| ctx.e(l, g, "irisShape")).unwrap_or(0);
        let blades = match shape {
            0 => 4,
            1..=8 => shape.saturating_add(2),
            _ => return Err(bad(CMD, "invalid iris shape")),
        };
        let dof = opts.is_some_and(|(l, g)| ctx.b(l, g, "dof")) && aperture > 0.0 && value("blurLevel", 100.0) > 0.0;
        let source_time = ctx.source_time(layer);
        let source_frame = if footage.kind == FootageKind::Still {
            0
        } else {
            footage.frame_rate.frame_at(source_time).clamp(0, source_count.saturating_mul(footage.loop_count.max(1) as i64).saturating_sub(1)) % source_count
        };
        finite(location.into_iter().chain([focus, aperture, zoom, fstop, rotation, ratio, value("blurLevel", 100.0), value("irisRoundness", 0.0)]))?;
        if focus <= 0.0 || !(0.01..=100.0).contains(&ratio) || !(0.01..=1e6).contains(&fstop) {
            return Err(bad(CMD, "focus, f-stop or iris aspect is outside Blender's supported range"));
        }
        frames.push(json!({"frame": start + i as i64, "timeTicks": time.0, "sourceFrame": source_frame,
            "location": location, "rotation": [right, up, back], "lens": lens, "focus": focus, "dof": dof,
            "fstop": fstop, "blades": blades, "irisRotation": rotation, "irisRatio": ratio,
            "sourceAperture": aperture, "sourceZoom": zoom,
            "blurLevel": value("blurLevel", 100.0), "irisRoundness": value("irisRoundness", 0.0)}));
    }
    Ok((frames, world))
}
