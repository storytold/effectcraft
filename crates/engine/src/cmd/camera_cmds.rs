//! 3D Camera Tracker commands: Animation ▸ Track Camera (`track.camera`), the effect's Analyze /
//! Cancel / Create Camera buttons, the viewer's track point selection and right-click menu
//! (Set Ground Plane and Origin, Create Text / Solid / Null / Shadow Catcher … and Camera,
//! Create Multiple …, Delete Selected Points) and the status / points queries.

use effectcraft_color::Label;
use effectcraft_effects::camera_tracker::{self as ct, CAMERA, DELETED, SOLVE};
use effectcraft_keyframe::{Justify, TextDoc, Value as KV};
use effectcraft_project::build;
use effectcraft_project::{ItemId, ItemKind, Layer, LayerId, LayerSource, LightKind, Project, Solid, Uid};
use effectcraft_track::camtrack::linalg::{self, V3};
use effectcraft_track::camtrack::{ShotType, SolveMethod, Target, point_color};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, f_p, has_comp, layer_p, str_p};
use crate::camera_track::{CreateKind, Placed, is_tracker, placed, plane_orientation, static_params, target_for, write_camera};
use crate::{EngineError, Result, Session, cmd, query};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.camera",
            "Track Camera",
            ["Animation"],
            None,
            "{layer?, shotType?: fixed|variable|specify, aov?: degrees, solveMethod?: auto|typical|flat|tripod, detailed?, lensDistortion?: solve k1/k2, undistort?: render undistorted, wait?}",
            can_track,
            track_camera
        ),
        cmd!("camera.analyze", "Analyze", [], None, "{layer?, effect?: uid|name|index, wait?: block until done}", has_comp, analyze),
        cmd!("camera.cancel", "Cancel", [], None, "{}", is_analyzing, |s, _| Ok(json!({"stopped": s.stop_camera()}))),
        query!("camera.solveStatus", "3D Camera Tracker Status", "{layer?, effect?}", status),
        query!("camera.points", "3D Camera Tracker Points", "{layer?, effect?, time?: comp seconds} → visible solved points", points),
        cmd!("camera.selectPoints", "Select Track Points", [], None, "{points: [id], add?, toggle?}", has_comp, select_points),
        cmd!(
            "camera.createFromSolve",
            "Create from Camera Solve",
            [],
            None,
            "{kind: text|solid|null|shadowCatcher|camera, points?: [id] (default: selected), target?: {center, normal, size?} (comp world), multiple?, layer?, effect?}",
            has_comp,
            create_from_solve
        ),
        cmd!("camera.create", "Create Camera", [], None, "{layer?, effect?}", has_comp, |s, p| create_from_solve(s, &with_kind(p, "camera"))),
        cmd!("camera.setGroundPlane", "Set Ground Plane and Origin", [], None, "{points?: [id] (default: selected), layer?, effect?}", has_comp, set_ground),
        cmd!("camera.deletePoints", "Delete Selected Points", [], None, "{points?: [id] (default: selected), layer?, effect?, wait?}", has_comp, delete_points),
    ]
}

fn with_kind(p: &Value, kind: &str) -> Value {
    let mut q = p.clone();
    if !q.is_object() {
        q = json!({});
    }
    q["kind"] = json!(kind);
    q
}

fn can_track(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    let c = s.active_comp().ok_or("no composition is open")?;
    match s.state.selected_layers.first().and_then(|l| c.layer(*l)) {
        Some(l) if l.has_video() && !l.switches.adjustment && !l.is_camera() && !l.is_light() => Ok(()),
        Some(_) => Err("Track Camera needs a footage or composition layer".into()),
        None => Err("select a layer to track".into()),
    }
}

fn is_analyzing(s: &Session) -> std::result::Result<(), String> {
    if s.is_camera_analyzing() { Ok(()) } else { Err("no 3D Camera Tracker analysis is running".into()) }
}

/// (comp, layer, effect uid) of a 3D Camera Tracker: `layer` / `effect` (uid, name or 1-based
/// index), else a selected one, else the first in the comp.
fn ct_p(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, LayerId, Uid)> {
    let cid = super::comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let candidates: Vec<LayerId> = match p.get("layer") {
        Some(v) => vec![super::resolve_layer(comp, v).ok_or_else(|| bad(cmd, format!("no layer {v}")))?],
        None => {
            let mut v: Vec<LayerId> = s.state.selected_props.iter().map(|(l, _)| *l).collect();
            v.extend(s.state.selected_layers.iter().copied());
            v.extend(comp.layers.iter().map(|l| l.id));
            v
        }
    };
    for lid in candidates {
        let Some(fx) = comp.layer(lid).and_then(Layer::effects) else { continue };
        let g = match p.get("effect") {
            Some(Value::Number(n)) => {
                let n = n.as_u64().unwrap_or(0);
                fx.groups().find(|g| g.uid == n).or_else(|| fx.groups().nth((n as usize).saturating_sub(1)))
            }
            Some(Value::String(name)) => fx.groups().find(|g| &g.name == name),
            _ => s
                .state
                .selected_props
                .iter()
                .find_map(|(l, u)| (*l == lid).then(|| fx.groups().find(|g| g.uid == *u && is_tracker(g))).flatten())
                .or_else(|| fx.groups().find(|g| is_tracker(g))),
        };
        if let Some(g) = g.filter(|g| is_tracker(g)) {
            return Ok((cid, lid, g.uid));
        }
        if p.get("layer").is_some() {
            break;
        }
    }
    Err(bad(cmd, "no 3D Camera Tracker found (apply Animation ▸ Track Camera first)"))
}

fn shot_p(p: &Value) -> Result<Option<ShotType>> {
    match p.get("shotType") {
        None => Ok(None),
        Some(Value::Number(n)) => ShotType::ALL.get(n.as_u64().unwrap_or(99) as usize).copied().map(Some).ok_or_else(|| bad("track.camera", "shotType: 0..2")),
        Some(Value::String(v)) => ShotType::from_name(v).map(Some).ok_or_else(|| bad("track.camera", "shotType: fixed|variable|specify")),
        Some(_) => Err(bad("track.camera", "shotType: fixed|variable|specify")),
    }
}

fn method_p(p: &Value) -> Result<Option<SolveMethod>> {
    match p.get("solveMethod") {
        None => Ok(None),
        Some(Value::Number(n)) => {
            SolveMethod::ALL.get(n.as_u64().unwrap_or(99) as usize).copied().map(Some).ok_or_else(|| bad("track.camera", "solveMethod: 0..3"))
        }
        Some(Value::String(v)) => SolveMethod::from_name(v).map(Some).ok_or_else(|| bad("track.camera", "solveMethod: auto|typical|flat|tripod")),
        Some(_) => Err(bad("track.camera", "solveMethod: auto|typical|flat|tripod")),
    }
}

/// Animation ▸ Track Camera / the Tracker panel button: apply the 3D Camera Tracker to the layer
/// (reusing one it already has), set the given options and start the analysis.
fn track_camera(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "track.camera")?;
    let shot = shot_p(p)?;
    let method = method_p(p)?;
    let aov = f_p(p, "aov").or_else(|| f_p(p, "angleOfView"));
    if aov.is_some_and(|a| !(1.0..=170.0).contains(&a)) {
        return Err(bad("track.camera", "aov: 1..170 degrees"));
    }
    let detailed = b_p(p, "detailed");
    let lens = b_p(p, "lensDistortion");
    let undistort = b_p(p, "undistort");
    let existing = s.project.comp(cid).and_then(|c| c.layer(lid)).and_then(Layer::effects).and_then(|fx| fx.groups().find(|g| is_tracker(g)).map(|g| g.uid));
    let uid = match existing {
        Some(u) => u,
        None => {
            let r = s.execute("effect.apply", json!({"effect": ct::ID, "layers": [lid.0], "comp": cid.0}))?;
            r["effects"].get(0).and_then(Value::as_u64).ok_or_else(|| EngineError::Other("the effect could not be applied".into()))?
        }
    };
    if shot.is_some() || method.is_some() || aov.is_some() || detailed.is_some() || lens.is_some() || undistort.is_some() {
        s.edit("3D Camera Tracker Settings", None, |proj, _| {
            let g = proj
                .comp_mut(cid)
                .and_then(|c| c.layer_mut(lid))
                .and_then(|l| l.props.find_group_mut(uid))
                .ok_or_else(|| EngineError::Other("no 3D Camera Tracker".into()))?;
            let mut set = |m: &str, v: KV| {
                if let Some(pr) = g.prop_mut(m) {
                    pr.keys.clear();
                    pr.value = v;
                }
            };
            if let Some(sh) = shot {
                set("shotType", KV::Enum(ShotType::ALL.iter().position(|x| *x == sh).unwrap_or(0) as u32));
            }
            if let Some(a) = aov {
                set("horizontalAngleOfView", KV::Scalar(a));
                if shot.is_none() {
                    set("shotType", KV::Enum(2));
                }
            }
            if let Some(m) = method {
                set("advanced/solveMethod", KV::Enum(SolveMethod::ALL.iter().position(|x| *x == m).unwrap_or(0) as u32));
            }
            if let Some(d) = detailed {
                set("advanced/detailedAnalysis", KV::Bool(d));
            }
            if let Some(d) = lens {
                set("advanced/lensDistortion", KV::Bool(d));
            }
            if let Some(d) = undistort {
                set("advanced/undistort", KV::Bool(d));
            }
            Ok(())
        })?;
    }
    // Select the effect so the viewer shows its track points.
    s.state.selected_props = vec![(lid, uid)];
    let wait = b_p(p, "wait").unwrap_or(false);
    let n = s.start_camera(cid, lid, uid, wait).map_err(EngineError::Other)?;
    let mut out = json!({"layer": lid.0, "effect": uid, "frames": n, "running": s.is_camera_analyzing(), "progress": s.camera_progress()});
    if wait {
        out["status"] = status(s, &json!({"layer": lid.0, "effect": uid, "comp": cid.0}))?;
    }
    Ok(out)
}

fn analyze(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = ct_p(s, p, "camera.analyze")?;
    let wait = b_p(p, "wait").unwrap_or(false);
    let n = s.start_camera(cid, lid, uid, wait).map_err(EngineError::Other)?;
    let mut out = json!({"layer": lid.0, "effect": uid, "frames": n, "running": s.is_camera_analyzing(), "progress": s.camera_progress()});
    if wait {
        out["status"] = status(s, &json!({"layer": lid.0, "effect": uid, "comp": cid.0}))?;
    }
    Ok(out)
}

fn status(s: &mut Session, p: &Value) -> Result<Value> {
    s.poll_camera(false);
    let mut out = json!({"running": s.is_camera_analyzing(), "progress": s.camera_progress(), "banner": s.camera_progress().map(|p| p.banner())});
    let Ok((cid, lid, uid)) = ct_p(s, p, "camera.solveStatus") else { return Ok(out) };
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let g = layer.props.find_group(uid).ok_or(EngineError::NoComp)?;
    let params = static_params(g);
    out["layer"] = json!(lid.0);
    out["effect"] = json!(uid);
    out["pending"] = json!(s.camera_pending.contains(&(cid, lid, uid)));
    let tracks = ct::tracks(&params);
    out["analyzed"] = json!(tracks.is_some());
    if let Some(t) = &tracks {
        out["tracks"] = json!(t.tracks.len());
        out["frames"] = json!(t.frames);
    }
    let settings = ct::settings(&params);
    out["shotType"] = json!(settings.shot.label());
    out["solveMethod"] = json!(settings.method.label());
    out["deleted"] = json!(settings.deleted.len());
    out["lensDistortionSolved"] = json!(settings.lens_distortion);
    let solve = ct::solve(&params);
    out["solved"] = json!(solve.is_some());
    if let Some(sv) = &solve {
        let f = sv.focal();
        out["methodUsed"] = json!(sv.method_used.label());
        out["averageError"] = json!(sv.average_error);
        out["points"] = json!(sv.points.len());
        out["solvedFrames"] = json!(sv.frames.iter().filter(|c| c.solved).count());
        out["focalLength"] = json!(f);
        out["horizontalAngleOfView"] = json!(sv.hfov(f));
        out["groundPlane"] = json!(sv.ground.is_some());
        out["lensDistortion"] = match &sv.distortion {
            Some(d) => json!({"k1": d.k1, "k2": d.k2, "radius": d.radius}),
            None => Value::Null,
        };
        out["undistort"] = json!(ct::undistorts(&params, sv));
        if let Some(pl) = placed(&s.project, cid, lid, uid) {
            let k = pl.frame_at(s.time_of(cid));
            let (pos, ori, zoom) = pl.camera(k);
            out["frame"] = json!(k);
            out["camera"] = json!({"position": pos, "orientation": ori, "zoom": zoom});
            out["visible"] = json!(sv.visible(k).count());
        }
    }
    let cam = params.get(CAMERA).map(|v| v.as_f64() as u64).unwrap_or(0);
    out["cameraLayer"] = if cam > 0 && comp.layer(LayerId(cam)).is_some_and(Layer::is_camera) { json!(cam) } else { Value::Null };
    out["selected"] = json!(s.state.camera_points);
    Ok(out)
}

/// The visible solved points at the current (or given) time, with their comp positions.
fn points(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = ct_p(s, p, "camera.points")?;
    let pl = placed(&s.project, cid, lid, uid).ok_or_else(|| bad("camera.points", "the camera is not solved yet"))?;
    let t = f_p(p, "time").map(effectcraft_time::Tick::from_seconds_f64).unwrap_or_else(|| s.time_of(cid));
    let k = pl.frame_at(t);
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, t);
    let m = ctx.world_matrix(layer);
    let list: Vec<Value> = ct::projected(&pl.solve, k)
        .into_iter()
        .map(|(id, q, z)| {
            let c = m.apply(effectcraft_geom::vec3(q[0], q[1], 0.0));
            let sp = pl.solve.point(id).copied().unwrap_or_default();
            let col = point_color(id);
            json!({"id": id, "comp": [c.x, c.y], "layer": q, "depth": z, "world": pl.to_world(sp.pos), "error": sp.error, "color": col, "selected": s.state.camera_points.contains(&id)})
        })
        .collect();
    Ok(json!({"frame": k, "points": list}))
}

fn ids_p(s: &Session, p: &Value) -> Vec<u32> {
    match p.get("points") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_u64).map(|v| v as u32).collect(),
        Some(Value::Number(n)) => n.as_u64().map(|v| vec![v as u32]).unwrap_or_default(),
        _ => s.state.camera_points.clone(),
    }
}

fn select_points(s: &mut Session, p: &Value) -> Result<Value> {
    let ids: Vec<u32> = match p.get("points") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_u64).map(|v| v as u32).collect(),
        _ => vec![],
    };
    if b_p(p, "toggle").unwrap_or(false) {
        for id in ids {
            match s.state.camera_points.iter().position(|x| *x == id) {
                Some(i) => {
                    s.state.camera_points.remove(i);
                }
                None => s.state.camera_points.push(id),
            }
        }
    } else if b_p(p, "add").unwrap_or(false) {
        for id in ids {
            if !s.state.camera_points.contains(&id) {
                s.state.camera_points.push(id);
            }
        }
    } else {
        s.state.camera_points = ids;
    }
    s.bump();
    Ok(json!({"selected": s.state.camera_points}))
}

/// The target of a create / ground command in canonical solve coordinates.
fn target_p(s: &Session, p: &Value, pl: &Placed, k: usize, cmd: &str) -> Result<Target> {
    if let Some(t) = p.get("target") {
        let v3 = |k: &str| -> Option<V3> {
            let a = t.get(k)?.as_array()?;
            Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2).and_then(Value::as_f64).unwrap_or(0.0)])
        };
        let (c, n) = (v3("center").ok_or_else(|| bad(cmd, "target.center: [x, y, z]"))?, v3("normal").unwrap_or([0.0, -1.0, 0.0]));
        let size = t.get("size").and_then(Value::as_f64).unwrap_or(100.0);
        let nc = linalg::normalize(linalg::mtv(&pl.world.r, n));
        return Ok(Target { center: pl.from_world(c), normal: nc, size: size / pl.world.s });
    }
    let ids = ids_p(s, p);
    if ids.is_empty() {
        return Err(bad(cmd, "select track points (or give `points` / `target`)"));
    }
    target_for(&pl.solve, &ids, k).ok_or_else(|| bad(cmd, "those points do not define a plane"))
}

fn next_name(c: &effectcraft_project::Comp, base: &str) -> String {
    let n = (1..).find(|i| !c.layers.iter().any(|l| l.name == format!("{base} {i}"))).unwrap_or(1);
    format!("{base} {n}")
}

fn place(l: &mut Layer, pos: V3, ori: V3, src: &Layer) {
    l.switches.three_d = true;
    l.in_point = src.in_point;
    l.out_point = src.out_point;
    if let Some(pr) = l.props.prop_mut("transform/position") {
        pr.value = KV::Vec3(pos);
    }
    if let Some(pr) = l.props.prop_mut("transform/orientation") {
        pr.value = KV::Vec3(ori);
    }
}

/// Insert `l` above the tracked layer.
fn insert_above(proj: &mut Project, cid: ItemId, above: LayerId, l: Layer) -> Result<LayerId> {
    let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
    let at = comp.layers.iter().position(|x| x.id == above).unwrap_or(0);
    let id = l.id;
    comp.layers.insert(at, l);
    Ok(id)
}

fn add_solid(proj: &mut Project, comp: &effectcraft_project::Comp, name: &str, size: u32, color: [f32; 3]) -> Layer {
    let folder = proj.folder_named("Solids").unwrap_or_else(|| proj.add_item("Solids", Label::Yellow, None, ItemKind::Folder));
    let sid = proj.add_item(name, Label::Red, Some(folder), ItemKind::Solid(Solid { color, width: size, height: size, pixel_aspect: 1.0 }));
    build::layer(proj, comp, name, LayerSource::Solid { item: sid }, (size, size), None)
}

/// Create Text / Solid / Null / Shadow Catcher (and Light) on the target, plus the 3D Tracker
/// Camera when the tracker has none yet — one undo step.
fn create_from_solve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "camera.createFromSolve";
    let (cid, lid, uid) = ct_p(s, p, cmd)?;
    let kind = CreateKind::from_name(str_p(p, "kind").unwrap_or("null")).ok_or_else(|| bad(cmd, "kind: text|solid|null|shadowCatcher|camera"))?;
    let pl = placed(&s.project, cid, lid, uid).ok_or_else(|| bad(cmd, "the camera is not solved yet (analysis still running or failed)"))?;
    let k = pl.frame_at(s.time_of(cid));
    let multiple = b_p(p, "multiple").unwrap_or(false);
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let src = comp.layer(lid).ok_or(EngineError::NoComp)?.clone();
    let g = src.props.find_group(uid).ok_or(EngineError::NoComp)?;
    let params = static_params(g);
    let size_pct = effectcraft_effects::warp_stab::param(&params, "targetSize").map(|v| v.as_f64()).unwrap_or(100.0) / 100.0;
    let existing_cam = params.get(CAMERA).map(|v| v.as_f64() as u64).filter(|id| *id > 0 && comp.layer(LayerId(*id)).is_some_and(Layer::is_camera));
    // Targets: one, or one per selected point (Create Multiple …) sharing the selection's plane.
    let targets: Vec<Target> = if kind == CreateKind::Camera {
        vec![]
    } else {
        let t = target_p(s, p, &pl, k, cmd)?;
        if multiple {
            let ids = ids_p(s, p);
            ids.iter().filter_map(|i| pl.solve.point(*i)).map(|sp| Target { center: sp.pos, ..t }).collect()
        } else {
            vec![t]
        }
    };
    // Camera right vector on this frame (world), to align created layers with the view.
    let cam = &pl.solve.frames[k];
    let right = linalg::mv(&pl.world.r, linalg::mtv(&cam.r(), [1.0, 0.0, 0.0]));
    let label = match (kind, multiple) {
        (CreateKind::Text, false) => "Create Text and Camera",
        (CreateKind::Solid, false) => "Create Solid and Camera",
        (CreateKind::Null, false) => "Create Null and Camera",
        (CreateKind::ShadowCatcher, _) => "Create Shadow Catcher, Camera and Light",
        (CreateKind::Text, true) => "Create Multiple Text Layers and Camera",
        (CreateKind::Solid, true) => "Create Multiple Solids and Camera",
        (CreateKind::Null, true) => "Create Multiple Nulls and Camera",
        (CreateKind::Camera, _) => "Create Camera",
    };
    let (camera, created) = s.edit(label, None, |proj, st| {
        let mut created = vec![];
        // The camera: reuse the tracker's, else create "3D Tracker Camera" at the top.
        let camera = match existing_cam {
            Some(id) if kind != CreateKind::Camera => LayerId(id),
            Some(id) => {
                let l = proj.comp_mut(cid).and_then(|c| c.layer_mut(LayerId(id))).ok_or(EngineError::NoComp)?;
                write_camera(l, &pl);
                LayerId(id)
            }
            None => {
                let mut l = build::layer(proj, &comp, "3D Tracker Camera", LayerSource::Camera, (comp.width, comp.height), None);
                l.in_point = src.in_point;
                l.out_point = src.out_point;
                write_camera(&mut l, &pl);
                let id = l.id;
                proj.comp_mut(cid).ok_or(EngineError::NoComp)?.layers.insert(0, l);
                if let Some(pr) = proj.comp_mut(cid).and_then(|c| c.layer_mut(lid)).and_then(|l| l.props.find_group_mut(uid)).and_then(|g| g.prop_mut(CAMERA)) {
                    pr.value = KV::Scalar(id.0 as f64);
                }
                id
            }
        };
        for t in &targets {
            let center = pl.to_world(t.center);
            let normal = linalg::normalize(linalg::mv(&pl.world.r, t.normal));
            let ori = plane_orientation(normal, right);
            let size = (t.size * pl.world.s * size_pct).max(1.0);
            let cur = proj.comp(cid).ok_or(EngineError::NoComp)?.clone();
            match kind {
                CreateKind::Text => {
                    let mut l = build::layer(proj, &cur, "Text", LayerSource::Text, (comp.width, comp.height), None);
                    let doc = TextDoc { text: "Text".into(), justify: Justify::Center, size: (size * 0.35).clamp(4.0, 2000.0), ..Default::default() };
                    if let Some(pr) = l.props.prop_mut("text/sourceText") {
                        pr.value = KV::Text(Box::new(doc));
                    }
                    place(&mut l, center, ori, &src);
                    created.push(insert_above(proj, cid, lid, l)?);
                }
                CreateKind::Solid => {
                    let name = next_name(&cur, "Track Solid");
                    let c = point_color((created.len() as u32).wrapping_add(7));
                    let mut l = add_solid(proj, &cur, &name, size.round().max(1.0) as u32, c);
                    place(&mut l, center, ori, &src);
                    created.push(insert_above(proj, cid, lid, l)?);
                }
                CreateKind::Null => {
                    let name = next_name(&cur, "Track Null");
                    let mut l = build::layer(proj, &cur, &name, LayerSource::Null, (comp.width, comp.height), None);
                    place(&mut l, center, ori, &src);
                    if let Some(pr) = l.props.prop_mut("transform/scale") {
                        pr.value = KV::Vec3([size, size, size]);
                    }
                    created.push(insert_above(proj, cid, lid, l)?);
                }
                CreateKind::ShadowCatcher => {
                    let name = next_name(&cur, "Shadow Catcher");
                    let mut l = add_solid(proj, &cur, &name, (size * 4.0).round().max(1.0) as u32, [1.0, 1.0, 1.0]);
                    place(&mut l, center, ori, &src);
                    if let Some(pr) = l.props.prop_mut("materialOptions/acceptsShadows") {
                        pr.value = KV::Enum(2);
                    }
                    if let Some(pr) = l.props.prop_mut("materialOptions/castsShadows") {
                        pr.value = KV::Enum(0);
                    }
                    let sc = created.len();
                    created.push(insert_above(proj, cid, lid, l)?);
                    let cur = proj.comp(cid).ok_or(EngineError::NoComp)?.clone();
                    let lname = next_name(&cur, "Light");
                    let mut light = build::layer(proj, &cur, &lname, LayerSource::Light { kind: LightKind::Point }, (comp.width, comp.height), None);
                    light.in_point = src.in_point;
                    light.out_point = src.out_point;
                    let lp = linalg::add(center, linalg::add(linalg::scale(normal, size * 2.0), linalg::scale(right, size)));
                    if let Some(pr) = light.props.prop_mut("transform/position") {
                        pr.value = KV::Vec3(lp);
                    }
                    if let Some(pr) = light.props.prop_mut("lightOptions/castsShadows") {
                        pr.value = KV::Bool(true);
                    }
                    let _ = sc;
                    created.push(insert_above(proj, cid, created[created.len() - 1], light)?);
                }
                CreateKind::Camera => {}
            }
        }
        st.selected_layers = if created.is_empty() { vec![camera] } else { created.clone() };
        st.selected_props.clear();
        Ok((camera, created))
    })?;
    let t0 = targets.first().map(
        |t| json!({"center": pl.to_world(t.center), "normal": linalg::normalize(linalg::mv(&pl.world.r, t.normal)), "size": t.size * pl.world.s * size_pct}),
    );
    Ok(json!({"camera": camera.0, "layers": created.iter().map(|l| l.0).collect::<Vec<_>>(), "target": t0}))
}

fn set_ground(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "camera.setGroundPlane";
    let (cid, lid, uid) = ct_p(s, p, cmd)?;
    let pl = placed(&s.project, cid, lid, uid).ok_or_else(|| bad(cmd, "the camera is not solved yet"))?;
    let k = pl.frame_at(s.time_of(cid));
    let ids = ids_p(s, p);
    if ids.is_empty() {
        return Err(bad(cmd, "select track points (or give `points`)"));
    }
    let t = target_for(&pl.solve, &ids, k).ok_or_else(|| bad(cmd, "those points do not define a plane"))?;
    let mut sv = (*pl.solve).clone();
    let mut g = sv.ground_from(&ids, k).ok_or_else(|| bad(cmd, "those points do not define a plane"))?;
    g.origin = t.center;
    sv.ground = Some(g);
    let json = sv.to_json();
    s.edit("Set Ground Plane and Origin", None, |proj, _| {
        let pr = proj
            .comp_mut(cid)
            .and_then(|c| c.layer_mut(lid))
            .and_then(|l| l.props.find_group_mut(uid))
            .and_then(|g| g.prop_mut(SOLVE))
            .ok_or_else(|| EngineError::Other("no 3D Camera Tracker".into()))?;
        pr.value = KV::Str(json);
        Ok(())
    })?;
    Ok(json!({"origin": g.origin, "normal": g.normal}))
}

/// Delete points (and, with Auto-delete Points Across Time, the points of other tracks on the
/// same 3D feature), then re-solve.
fn delete_points(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "camera.deletePoints";
    let (cid, lid, uid) = ct_p(s, p, cmd)?;
    let ids = ids_p(s, p);
    if ids.is_empty() {
        return Err(bad(cmd, "select track points (or give `points`)"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let g = comp.layer(lid).and_then(|l| l.props.find_group(uid)).ok_or(EngineError::NoComp)?;
    let params = static_params(g);
    let mut del = ct::deleted(&params);
    let auto = effectcraft_effects::warp_stab::param(&params, "advanced/autoDeletePoints").is_some_and(KV::as_bool);
    let mut add = ids.clone();
    if auto && let Some(sv) = ct::solve(&params) {
        // Same feature tracked again at other times: solved within 1 % of its depth.
        let cam0 = sv.first_solved().map(|k| sv.frames[k].center).unwrap_or([0.0; 3]);
        for id in &ids {
            let Some(pt) = sv.point(*id) else { continue };
            let r = linalg::norm(linalg::sub(pt.pos, cam0)) * 0.01;
            for q in &sv.points {
                let disjoint = q.last < pt.first || q.first > pt.last;
                if q.id != *id && disjoint && linalg::norm(linalg::sub(q.pos, pt.pos)) < r {
                    add.push(q.id);
                }
            }
        }
    }
    for id in add {
        if !del.contains(&id) {
            del.push(id);
        }
    }
    del.sort_unstable();
    let json = serde_json::to_string(&del).unwrap_or_default();
    s.edit("Delete Track Points", None, |proj, _| {
        let pr = proj
            .comp_mut(cid)
            .and_then(|c| c.layer_mut(lid))
            .and_then(|l| l.props.find_group_mut(uid))
            .and_then(|g| g.prop_mut(DELETED))
            .ok_or_else(|| EngineError::Other("no 3D Camera Tracker".into()))?;
        pr.value = KV::Str(json);
        Ok(())
    })?;
    s.state.camera_points.retain(|x| !del.contains(x));
    // Re-solve now (After Effects re-solves after deleting points).
    let wait = b_p(p, "wait").unwrap_or(false);
    if !s.is_camera_analyzing() {
        s.start_camera(cid, lid, uid, wait).map_err(EngineError::Other)?;
    }
    Ok(json!({"deleted": del, "running": s.is_camera_analyzing()}))
}
