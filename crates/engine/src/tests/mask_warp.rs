//! Mask tracking, Mask Interpolation and the Warp Stabilizer end to end through the commands, on
//! synthetic footage generated in-test (a rotating/scaling textured patch, a shaky camera).

use effectcraft_keyframe::{Keyframe, ShapePath, Value as KV};
use effectcraft_path::interp::paths_cross;
use effectcraft_project::{LayerId, Uid};
use effectcraft_raster::Image;
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;
use effectcraft_track::Homography;
use serde_json::json;

use crate::Session;
use crate::tests::track::{FPS, H, W, frame_time, patch_frames, rotating_pose, setup, texture};

fn pose_h(f: u32) -> Homography {
    let (c, rot, sc) = rotating_pose(f);
    let (sn, cs) = rot.to_radians().sin_cos();
    Homography([[sc * cs, -sc * sn, c[0]], [sc * sn, sc * cs, c[1]], [0.0, 0.0, 1.0]])
}

fn mask_path(s: &Session, layer: LayerId, mask: Uid) -> effectcraft_project::Property {
    let l = s.active_comp().unwrap().layer(layer).unwrap();
    l.props.find_group(mask).unwrap().get("path").unwrap().clone()
}

fn new_mask(s: &mut Session, layer: LayerId, path: &ShapePath) -> Uid {
    let ins: Vec<[f64; 2]> = path.in_tangents.clone();
    let outs: Vec<[f64; 2]> = path.out_tangents.clone();
    let r = s.execute("mask.new", json!({"layer": layer.0, "vertices": path.vertices, "inTangents": ins, "outTangents": outs, "closed": true})).unwrap();
    r["mask"].as_u64().unwrap()
}

#[test]
fn track_mask_follows_a_rotating_patch() {
    let (mut s, clip, _) = setup(patch_frames(rotating_pose));
    let (c0, _, _) = rotating_pose(0);
    // An ellipse inside the textured patch.
    let shape = ShapePath::ellipse(c0, 64.0, 52.0);
    let uid = new_mask(&mut s, clip, &shape);
    // Selecting the mask enables Track Mask (the Tracker panel's mask mode).
    s.execute("layer.select", json!({"layers": [clip.0]})).unwrap();
    s.state.selected_props = vec![(clip, uid)];
    assert!(s.is_enabled("track.mask"));
    assert_eq!(crate::commands::mask_interp::selected_mask(&s), Some((clip, uid)));
    s.execute("time.set", json!({"time": 0})).unwrap();
    let r = s.execute_checked("track.mask", json!({"method": "positionScaleRotation", "direction": "forward", "wait": true})).unwrap();
    assert_eq!(r["frames"], 24, "{r}");
    assert!(s.mask_job.is_none());
    let pr = mask_path(&s, clip, uid);
    assert_eq!(pr.keys.len(), 25);
    let inv0 = pose_h(0).inverse().unwrap();
    let mut worst: f64 = 0.0;
    for f in 0..25 {
        let KV::Path(p) = pr.value_at(frame_time(f)) else { panic!() };
        let m = pose_h(f).then_after(&inv0);
        for (v, o) in p.vertices.iter().zip(&shape.vertices) {
            let want = m.apply(*o);
            worst = worst.max((v[0] - want[0]).hypot(v[1] - want[1]));
        }
        // Tangents rotate and scale with the patch.
        let want_t = m.apply_tangent(shape.vertices[0], shape.out_tangents[0]);
        assert!((p.out_tangents[0][0] - want_t[0]).hypot(p.out_tangents[0][1] - want_t[1]) < 1.5);
    }
    assert!(worst < 1.5, "worst vertex error {worst} px");
    assert_eq!(s.time(), frame_time(24));
    // One undo step; redo restores the keys.
    assert!(s.undo());
    assert!(mask_path(&s, clip, uid).keys.is_empty());
    assert!(s.redo());
    assert_eq!(mask_path(&s, clip, uid).keys.len(), 25);
    // Track backward one frame from the end, with another method.
    let r = s.execute("track.mask", json!({"layer": clip.0, "mask": uid, "method": "position", "direction": "frameBackward", "wait": true})).unwrap();
    assert_eq!(r["frames"], 1);
    assert_eq!(s.state.mask_track_method, crate::mask_track::MaskMethod::Position);
    // Serde keeps the keys.
    let back = effectcraft_project::Project::from_json(&s.project.to_json()).unwrap();
    let l = back.comp(s.active_comp_id().unwrap()).unwrap().layer(clip).unwrap();
    assert_eq!(l.props.find_group(uid).unwrap().get("path").unwrap().keys.len(), 25);
    // Bad parameters are reported.
    assert!(s.execute_checked("track.mask", json!({"layer": clip.0, "mask": uid, "method": "warp"})).is_err());
    assert!(s.execute_checked("track.mask", json!({"layer": clip.0, "mask": uid, "bogus": 1})).is_err());
}

#[test]
fn track_mask_in_background_and_cancel() {
    let (mut s, clip, _) = setup(patch_frames(rotating_pose));
    let uid = new_mask(&mut s, clip, &ShapePath::rect(rotating_pose(0).0, 60.0, 60.0));
    s.execute("time.set", json!({"time": 0})).unwrap();
    let r = s.execute("track.mask", json!({"layer": clip.0, "mask": uid, "method": "perspective"})).unwrap();
    assert_eq!(r["running"], true);
    // The analysis keeps running in the background; Stop keeps what was tracked.
    assert!(s.is_enabled("track.stop"));
    s.execute("track.stop", json!({})).unwrap();
    let t0 = std::time::Instant::now();
    while s.mask_job.is_some() {
        assert!(t0.elapsed().as_secs() < 60);
        std::thread::sleep(std::time::Duration::from_millis(2));
        s.poll_mask_track();
    }
    let n = mask_path(&s, clip, uid).keys.len();
    assert!((1..25).contains(&n), "{n} keys");
    // A full background run.
    s.execute("time.set", json!({"time": 0})).unwrap();
    s.execute("track.mask", json!({"layer": clip.0, "mask": uid, "method": "perspective"})).unwrap();
    while s.mask_job.is_some() {
        assert!(t0.elapsed().as_secs() < 120);
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.poll_mask_track();
    }
    let pr = mask_path(&s, clip, uid);
    assert_eq!(pr.keys.len(), 25);
    let KV::Path(p) = pr.value_at(frame_time(20)) else { panic!() };
    let m = pose_h(20).then_after(&pose_h(0).inverse().unwrap());
    let want = m.apply(ShapePath::rect(rotating_pose(0).0, 60.0, 60.0).vertices[2]);
    assert!((p.vertices[2][0] - want[0]).hypot(p.vertices[2][1] - want[1]) < 2.0);
}

/// A five-pointed star (10 vertices) around `c`, rotated by `rot` degrees.
fn star(c: [f64; 2], r: f64, rot: f64) -> ShapePath {
    let pts: Vec<[f64; 2]> = (0..10)
        .map(|i| {
            let a = (rot + i as f64 * 36.0 - 90.0).to_radians();
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            [c[0] + rr * a.cos(), c[1] + rr * a.sin()]
        })
        .collect();
    ShapePath::polygon(&pts, true)
}

#[test]
fn mask_interpolation_creates_matching_keys() {
    let (mut s, clip, _) = setup(patch_frames(rotating_pose));
    let a = ShapePath::rect([100.0, 100.0], 80.0, 60.0);
    let b = star([200.0, 120.0], 50.0, 30.0);
    let uid = new_mask(&mut s, clip, &a);
    let cid = s.active_comp_id().unwrap();
    let (t0, t1) = (frame_time(0), frame_time(20));
    s.edit("keys", None, |p, _| {
        let pr = p.comp_mut(cid).unwrap().layer_mut(clip).unwrap().props.find_group_mut(uid).unwrap().get_mut("path").unwrap();
        pr.keys = vec![Keyframe::new(t0, KV::Path(a.clone())), Keyframe::new(t1, KV::Path(b.clone()))];
        Ok(())
    })
    .unwrap();
    let prop = mask_path(&s, clip, uid).uid;
    s.execute("keys.select", json!({"keys": [{"layer": clip.0, "prop": prop, "time": 0.0}, {"layer": clip.0, "prop": prop, "time": 0.8}]})).unwrap();
    assert_eq!(s.state.selected_keys.len(), 2);
    let r = s
        .execute_checked(
            "mask.interpolate",
            json!({"keyframeRate": "auto", "bendingResistance": 50, "quality": 50, "addVertices": 10, "addVerticesUnit": "pixels", "matchingMethod": "auto", "firstVerticesMatch": false}),
        )
        .unwrap();
    // 19 in-betweens at the comp rate (frames 1…19).
    assert_eq!(r["keys"], 19, "{r}");
    let pr = mask_path(&s, clip, uid);
    assert_eq!(pr.keys.len(), 21);
    let shapes: Vec<ShapePath> = pr.keys.iter().map(|k| k.value.as_path().unwrap().clone()).collect();
    let n = shapes[0].len();
    assert!(n >= 28, "{n} vertices");
    assert!(shapes.iter().all(|p| p.len() == n));
    // Ends keep their outlines; vertex paths between consecutive keys don't cross.
    for (i, w) in shapes.windows(2).enumerate() {
        assert_eq!(paths_cross(&w[0], &w[1]), 0, "keys {i}/{}", i + 1);
    }
    assert!(
        shapes[0]
            .vertices
            .iter()
            .all(|v| (v[0] - 60.0).abs() < 1e-6 || (v[0] - 140.0).abs() < 1e-6 || (v[1] - 70.0).abs() < 1e-6 || (v[1] - 130.0).abs() < 1e-6)
    );
    // Options persist; Keyframe Fields doubles the rate; undo restores.
    assert_eq!(s.state.mask_interp.add_vertices, Some(10.0));
    assert!(s.undo());
    assert_eq!(mask_path(&s, clip, uid).keys.len(), 2);
    let r = s
        .execute(
            "mask.interpolate",
            json!({"layer": clip.0, "mask": uid, "times": [0.0, 0.8], "keyframeRate": 10, "keyframeFields": true, "linearVertexPaths": true}),
        )
        .unwrap();
    assert_eq!(r["keys"], 15, "{r}");
    assert!(s.execute("mask.interpolate", json!({"layer": clip.0, "mask": uid, "times": [0.0]})).is_err());
    assert!(s.execute_checked("mask.interpolate", json!({"matchingMethod": "magic", "times": [0, 0.8], "layer": clip.0})).is_err());
}

// ---------------------------------------------------------------- Warp Stabilizer

/// Camera offset on frame `f`: a slow pan plus jitter.
fn cam(f: u32) -> [f64; 2] {
    let j = [((f * 7919) % 9) as f64 - 4.0, ((f * 104_729) % 7) as f64 - 3.0];
    [1.2 * f as f64 + j[0], 0.5 * f as f64 + j[1]]
}

fn shaky(f: u32) -> Image {
    let c = cam(f);
    let mut img = Image::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let v = texture(x as f64 + 0.5 + c[0], y as f64 + 0.5 + c[1], 11);
            img.set(x, y, [v, v * 0.9, v * 0.8, 1.0]);
        }
    }
    img
}

fn jitter(p: &[[f64; 2]]) -> f64 {
    let mut s = 0.0;
    for k in 1..p.len() - 1 {
        s += (p[k + 1][0] - 2.0 * p[k][0] + p[k - 1][0]).hypot(p[k + 1][1] - 2.0 * p[k][1] + p[k - 1][1]);
    }
    s / (p.len() - 2) as f64
}

fn warp_at(s: &mut Session, f: u32) -> Homography {
    s.execute("time.set", json!({"time": frame_time(f).seconds()})).unwrap();
    let st = s.execute("warp.status", json!({})).unwrap();
    let m: [[f64; 3]; 3] = serde_json::from_value(st["warp"].clone()).unwrap_or_else(|_| panic!("{st}"));
    Homography(m)
}

/// Output positions of the scene point that sits at the frame centre on frame 0.
fn scene_track(s: &mut Session) -> (Vec<[f64; 2]>, Vec<[f64; 2]>) {
    let x0 = [160.0 + cam(0)[0], 120.0 + cam(0)[1]];
    let before: Vec<[f64; 2]> = (0..25).map(|f| [x0[0] - cam(f)[0], x0[1] - cam(f)[1]]).collect();
    let after = (0..25).map(|f| warp_at(s, f).apply(before[f as usize])).collect();
    (before, after)
}

fn render(s: &Session, f: u32) -> Image {
    s.render(s.active_comp_id().unwrap(), frame_time(f), RenderOpts::default())
}

fn transparent(img: &Image) -> usize {
    img.data.iter().filter(|p| p[3] < 0.5).count()
}

fn set_param(s: &mut Session, layer: LayerId, path: &str, v: serde_json::Value) {
    s.execute("prop.set", json!({"layer": layer.0, "path": format!("effects/#1/{path}"), "value": v})).unwrap();
}

#[test]
fn warp_stabilizer_analyzes_and_stabilizes() {
    let (mut s, clip, solid) = setup(shaky);
    s.execute("edit.clear", json!({"layers": [solid.0]})).unwrap();
    s.execute("layer.select", json!({"layers": [clip.0]})).unwrap();
    // Effect ▸ Distort ▸ Warp Stabilizer, then analyse (blocking).
    let r = s.execute("effect.apply", json!({"effect": "Warp Stabilizer"})).unwrap();
    let fx = r["effects"][0].as_u64().unwrap();
    assert!(s.warp_pending.contains(&(s.active_comp_id().unwrap(), clip, fx)), "applying queues an analysis");
    let st = s.execute_checked("warp.status", json!({})).unwrap();
    assert_eq!(st["analyzed"], false);
    let r = s.execute_checked("warp.analyze", json!({"layer": clip.0, "wait": true})).unwrap();
    assert_eq!(r["frames"], 25, "{r}");
    assert_eq!(r["status"]["analyzed"], true, "{r}");
    assert!(s.warp_pending.is_empty() && s.warp_job.is_none());
    // Groups and AE parameter names.
    let g = s.active_comp().unwrap().layer(clip).unwrap().props.find_group(fx).unwrap().clone();
    let names: Vec<&str> = g.groups().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["Stabilization", "Borders", "Advanced"]);
    assert!(g.prop("borders/autoScale/maximumScale").is_some());
    assert_eq!(g.prop("stabilization/smoothness").unwrap().name, "Smoothness");

    // Smooth Motion: the jitter of a scene point drops by more than 80 %.
    set_param(&mut s, clip, "borders/framing", json!(0));
    let (before, after) = scene_track(&mut s);
    let (j0, j1) = (jitter(&before), jitter(&after));
    assert!(j1 < 0.2 * j0, "jitter {j0} → {j1}");
    // Rendering applies the frame's correction: output(p) = source(W⁻¹ p).
    let w = warp_at(&mut s, 7);
    let out = render(&s, 7);
    let src = shaky(7);
    let inv = w.inverse().unwrap();
    for p in [[160.0, 120.0], [100.0, 80.0], [230.0, 170.0]] {
        let q = inv.apply([p[0] + 0.5, p[1] + 0.5]);
        let want = src.sample_bilinear(q[0], q[1]);
        let got = out.get(p[0] as i64, p[1] as i64);
        assert!((want[0] - got[0]).abs() < 0.03, "{p:?}: {want:?} vs {got:?}");
    }
    // Stabilize Only shows the moving borders.
    let shown: usize = (0..25).map(|f| transparent(&render(&s, f))).sum();
    assert!(shown > 500, "{shown}");

    // No Motion keeps a feature fixed.
    set_param(&mut s, clip, "stabilization/result", json!(1));
    let (_, fixed) = scene_track(&mut s);
    for q in &fixed {
        assert!((q[0] - fixed[12][0]).hypot(q[1] - fixed[12][1]) < 0.6, "{q:?} vs {:?}", fixed[12]);
    }
    set_param(&mut s, clip, "stabilization/result", json!(0));

    // Stabilize, Crop, Auto-scale (the default) hides the borders; Crop leaves an opaque centre.
    set_param(&mut s, clip, "borders/framing", json!(2));
    let st = s.execute("warp.status", json!({})).unwrap();
    assert!(st["autoScale"].as_f64().unwrap() > 100.5, "{st}");
    for f in 0..25 {
        assert_eq!(transparent(&render(&s, f)), 0, "frame {f}");
    }
    set_param(&mut s, clip, "borders/framing", json!(1));
    let st = s.execute("warp.status", json!({})).unwrap();
    let crop: [f64; 4] = serde_json::from_value(st["crop"].clone()).unwrap();
    for f in [0, 9, 24] {
        let img = render(&s, f);
        for y in (crop[1].ceil() as u32 + 1)..(crop[3].floor() as u32 - 1) {
            for x in (crop[0].ceil() as u32 + 1)..(crop[2].floor() as u32 - 1) {
                assert!(img.get(x as i64, y as i64)[3] > 0.99, "frame {f} ({x}, {y})");
            }
        }
        assert!(img.get(0, 0)[3] < 0.01);
    }
    // Synthesize Edges fills most of the borders from neighbouring frames.
    set_param(&mut s, clip, "borders/framing", json!(3));
    let synth: usize = (0..25).map(|f| transparent(&render(&s, f))).sum();
    assert!(synth * 10 < shown, "synthesized {synth} vs {shown}");
    // Show Track Points draws the features.
    set_param(&mut s, clip, "borders/framing", json!(0));
    let plain = render(&s, 5);
    set_param(&mut s, clip, "advanced/showTrackPoints", json!(true));
    assert_ne!(render(&s, 5).data, plain.data);

    // Serde keeps the analysis; undo / redo of the analysis.
    let back = effectcraft_project::Project::from_json(&s.project.to_json()).unwrap();
    let bg = back.comp(s.active_comp_id().unwrap()).unwrap().layer(clip).unwrap().props.find_group(fx).unwrap().clone();
    assert_eq!(bg, s.active_comp().unwrap().layer(clip).unwrap().props.find_group(fx).unwrap().clone());
    while s.history.undo.last().is_some_and(|(l, _)| l != "Warp Stabilizer Analysis") {
        s.undo();
    }
    assert!(s.undo());
    assert_eq!(s.execute("warp.status", json!({})).unwrap()["analyzed"], false);
    assert!(s.redo());
    assert_eq!(s.execute("warp.status", json!({})).unwrap()["analyzed"], true);
}

#[test]
fn warp_analysis_invalidates_and_cancels() {
    let (mut s, clip, solid) = setup(shaky);
    s.execute("edit.clear", json!({"layers": [solid.0]})).unwrap();
    s.execute("layer.select", json!({"layers": [clip.0]})).unwrap();
    // Animation ▸ Warp Stabilizer VFX applies and analyses.
    assert!(s.is_enabled("track.warpStabilizer"));
    let r = s.execute_checked("track.warpStabilizer", json!({"wait": true})).unwrap();
    let fx = r["effect"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    assert_eq!(s.execute("warp.status", json!({})).unwrap()["analyzed"], true);
    // Trimming the layer clears the analysis and queues it again.
    s.execute("layer.timing", json!({"layers": [clip.0], "out": 0.8})).unwrap();
    let st = s.execute("warp.status", json!({})).unwrap();
    assert_eq!(st["analyzed"], false, "{st}");
    assert_eq!(st["pending"], true);
    // Without an analysis the layer renders unstabilized.
    let img = s.render(cid, frame_time(3), RenderOpts::default());
    assert_eq!(transparent(&img), 0);
    // Frontends start queued analyses in the background (poll_warp with auto).
    assert!(s.poll_warp(true));
    assert!(s.warp_job.is_some());
    let t0 = std::time::Instant::now();
    while s.warp_job.is_some() {
        assert!(t0.elapsed().as_secs() < 120);
        let p = s.warp_progress().unwrap();
        assert!(p.banner().starts_with("Analyzing in background (step 1 of 2)") || p.banner() == "Stabilizing...");
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.poll_warp(true);
    }
    assert_eq!(s.execute("warp.status", json!({})).unwrap()["analyzed"], true);
    // Undoing the slip brings back the old analysis (the key matches again).
    // Cancel: nothing is written.
    s.execute("effect.apply", json!({"effect": "ec.distort.warpstabilizer", "layers": [clip.0]})).unwrap();
    let fx2 = s.active_comp().unwrap().layer(clip).unwrap().effects().unwrap().groups().nth(1).unwrap().uid;
    let r = s.execute("warp.analyze", json!({"layer": clip.0, "effect": fx2})).unwrap();
    assert_eq!(r["running"], true);
    assert!(s.execute("warp.analyze", json!({"layer": clip.0, "effect": fx})).is_err(), "one analysis at a time");
    s.execute("warp.cancel", json!({})).unwrap();
    while s.warp_job.is_some() {
        std::thread::sleep(std::time::Duration::from_millis(2));
        s.poll_warp(false);
    }
    let st = s.execute("warp.status", json!({"layer": clip.0, "effect": fx2})).unwrap();
    assert_eq!(st["analyzed"], false, "{st}");
    // Time Remap changes invalidate too.
    s.execute("layer.enableTimeRemap", json!({"layers": [clip.0]})).unwrap();
    assert_eq!(s.execute("warp.status", json!({"layer": clip.0, "effect": fx})).unwrap()["analyzed"], false);
    let _ = FPS;
    let _ = Tick::ZERO;
}

/// Jobs recorded by the session (the browser's job workers), run on demand in a second session
/// through JSON.
#[derive(Default)]
struct Deferred(std::sync::Mutex<Vec<(crate::offload::WorkerRequest, std::sync::Arc<crate::offload::Inbox>)>>);

impl crate::offload::Offload for Deferred {
    fn start(&self, req: crate::offload::WorkerRequest, inbox: std::sync::Arc<crate::offload::Inbox>) -> Result<(), String> {
        self.0.lock().unwrap().push((req, inbox));
        Ok(())
    }
    fn cancel(&self, _: u64) {}
}

impl Deferred {
    fn kinds(&self) -> Vec<crate::offload::JobKind> {
        self.0.lock().unwrap().iter().map(|(r, _)| r.job.kind()).collect()
    }
    /// Run the recorded jobs in a worker session (sharing `footage`).
    fn run(&self, footage: std::sync::Arc<dyn effectcraft_render::FootageSource>) {
        let jobs = std::mem::take(&mut *self.0.lock().unwrap());
        for (req, inbox) in jobs {
            let req: crate::offload::WorkerRequest = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
            let mut w = Session { footage: footage.clone(), ..Default::default() };
            let post: crate::offload::Post = std::rc::Rc::new(move |r| inbox.push(serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap()));
            crate::offload::run_request(&mut w, req, &post);
        }
    }
}

/// #1249: a setting changed while the analysis runs in a worker survives its result, which
/// used to replace the whole effect with the copy the job started from.
#[test]
fn an_edit_made_during_an_offloaded_analysis_survives_its_result() {
    let (mut s, clip, solid) = setup(shaky);
    s.execute("edit.clear", json!({"layers": [solid.0]})).unwrap();
    s.execute("layer.select", json!({"layers": [clip.0]})).unwrap();
    s.execute("effect.apply", json!({"effect": "Warp Stabilizer"})).unwrap();
    set_param(&mut s, clip, "stabilization/smoothness", json!(37.5));
    let off = std::sync::Arc::new(Deferred::default());
    s.offload = Some(off.clone());
    s.execute_checked("warp.analyze", json!({"layer": clip.0})).unwrap();
    // While the job runs: another setting of the same effect.
    set_param(&mut s, clip, "stabilization/smoothness", json!(12.5));
    let fx = |s: &Session| s.active_comp().unwrap().layer(clip).unwrap().props.prop("effects/#1/stabilization/smoothness").unwrap().value.clone();
    off.run(s.footage.clone());
    assert!(s.poll_offload());
    assert!(!s.drain_events().iter().any(|e| matches!(e, crate::Event::Toast { error: true, .. })), "applied without a conflict");
    let st = s.execute("warp.status", json!({})).unwrap();
    assert_eq!(st["analyzed"], true, "the analysis landed: {st}");
    assert_eq!(fx(&s), KV::Scalar(12.5), "the edit made during the analysis stays");
    // One undo step takes the analysis back and keeps the edit.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(fx(&s), KV::Scalar(12.5));
    assert_eq!(s.execute("warp.status", json!({})).unwrap()["analyzed"], false);
}

fn point_track_pose(f: u32) -> ([f64; 2], f64, f64) {
    ([150.0 + 2.0 * f as f64, 110.0 + f as f64], 0.0, 1.0)
}

/// Hold an actual point-tracking worker request so commands can edit its target before the
/// serialized worker reply is delivered.
fn held_point_track() -> (Session, LayerId, Uid, std::sync::Arc<Deferred>) {
    let (mut s, clip, solid) = setup(patch_frames(point_track_pose));
    s.execute("edit.clear", json!({"layers": [solid.0]})).unwrap();
    s.execute("layer.select", json!({"layers": [clip.0]})).unwrap();
    let tracker = s.execute("track.motion", json!({})).unwrap()["tracker"].as_u64().unwrap();
    s.execute("track.setPoint", json!({"point": 1, "center": point_track_pose(0).0, "featureSize": [36, 36], "searchSize": [80, 80]})).unwrap();
    let off = std::sync::Arc::new(Deferred::default());
    s.offload = Some(off.clone());
    s.execute_checked("track.analyze", json!({"direction": "forward"})).unwrap();
    assert_eq!(off.kinds(), [crate::offload::JobKind::Track]);
    (s, clip, tracker, off)
}

fn tracked_center(s: &Session, clip: LayerId, tracker: Uid) -> Uid {
    s.active_comp().unwrap().layer(clip).unwrap().props.find_group(tracker).unwrap().track_points().next().unwrap().get("featureCenter").unwrap().uid
}

#[test]
fn offloaded_conflict_preserves_state_and_reports_completion_failure() {
    let (mut s, clip, tracker, off) = held_point_track();
    let center = tracked_center(&s, clip, tracker);
    s.execute("prop.set", json!({"layer": clip.0, "prop": center, "value": [180.0, 140.0]})).unwrap();
    s.execute("prop.renameGroup", json!({"layer": clip.0, "prop": tracker, "name": "Redo survives"})).unwrap();
    assert!(s.undo());
    let before = s.project.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let steps = |s: &Session| {
        let entries =
            |v: &[(String, std::sync::Arc<effectcraft_project::Project>)]| v.iter().map(|(label, p)| (label.clone(), p.to_json())).collect::<Vec<_>>();
        (
            entries(&s.history.undo),
            entries(&s.history.redo),
            s.history.branches.iter().map(|b| (b.parent.to_json(), entries(&b.steps))).collect::<Vec<_>>(),
            s.history.merge_key.clone(),
        )
    };
    let history = steps(&s);
    let inbox = s.offloaded(crate::offload::JobKind::Track).unwrap().inbox.clone();
    off.run(s.footage.clone());
    let (done, replies): (Vec<_>, Vec<_>) = inbox.take().into_iter().partition(|r| matches!(r, crate::offload::WorkerReply::Done));
    assert_eq!(done.len(), 1);
    for reply in replies {
        inbox.push(reply);
    }
    s.drain_events();
    assert!(s.poll_offload());
    let pending = s.offloaded(crate::offload::JobKind::Track).unwrap();
    assert!(pending.progress.error.as_ref().is_some_and(|e| e.contains("nothing was applied")));
    assert!(!pending.progress.finished);
    assert!(s.offload_log.is_empty(), "the error is kept before completion is logged");
    // The current completion remains identifiable after the bounded log evicts an old entry.
    s.offload_log = vec![(crate::offload::JobKind::Warp, None); 32];
    for reply in done {
        inbox.push(reply);
    }
    assert!(s.poll_offload());
    assert!(std::sync::Arc::ptr_eq(&s.project, &before), "a rejected result changes no document content");
    assert_eq!(serde_json::to_value(&s.state).unwrap(), state);
    assert_eq!(steps(&s), history);
    let errors: Vec<_> = s
        .drain_events()
        .into_iter()
        .filter_map(|e| match e {
            crate::Event::Toast { error: true, message } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 1, "the apply rejection is reported once: {errors:?}");
    assert!(errors[0].contains("nothing was applied"));
    assert_eq!(s.offload_log.len(), 32);
    assert_eq!(s.offload_log_start, 1);
    let (kind, error) = s.offload_log.last().unwrap();
    assert_eq!(*kind, crate::offload::JobKind::Track);
    assert!(error.as_ref().is_some_and(|e| e.contains("nothing was applied")), "a rejected result must report failure to JobWaiter");
    assert!(s.redo());
    assert_eq!(s.active_comp().unwrap().layer(clip).unwrap().props.find_group(tracker).unwrap().name, "Redo survives");
}

#[test]
fn offloaded_disjoint_edit_reports_success_and_keeps_one_undo_step() {
    let (mut s, clip, tracker, off) = held_point_track();
    s.execute("prop.renameGroup", json!({"layer": clip.0, "prop": tracker, "name": "Live track"})).unwrap();
    s.execute("prop.setGroupEnabled", json!({"layer": clip.0, "prop": tracker, "value": false})).unwrap();
    let undo = s.history.undo.len();
    off.run(s.footage.clone());
    s.drain_events();
    assert!(s.poll_offload());
    let layer = s.active_comp().unwrap().layer(clip).unwrap();
    let track = layer.props.find_group(tracker).unwrap();
    assert_eq!(track.name, "Live track");
    assert!(!track.enabled);
    assert_eq!(track.track_points().next().unwrap().get("featureCenter").unwrap().keys.len(), 25);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(s.offload_log.last(), Some(&(crate::offload::JobKind::Track, None)));
    assert!(!s.drain_events().iter().any(|e| matches!(e, crate::Event::Toast { error: true, .. })));
    let applied = s.project.to_json();
    assert!(s.undo());
    let track = s.active_comp().unwrap().layer(clip).unwrap().props.find_group(tracker).unwrap();
    assert_eq!(track.name, "Live track");
    assert!(!track.enabled);
    assert!(track.track_points().next().unwrap().get("featureCenter").unwrap().keys.is_empty());
    assert!(s.redo());
    assert_eq!(s.project.to_json(), applied);
}

#[test]
fn offloaded_conflict_does_not_fail_a_concurrent_analysis() {
    let (mut s, clip, tracker, off) = held_point_track();
    let effect = s.execute("effect.apply", json!({"effect": "Warp Stabilizer", "layers": [clip.0]})).unwrap()["effects"][0].as_u64().unwrap();
    s.execute_checked("warp.analyze", json!({"layer": clip.0, "effect": effect})).unwrap();
    assert_eq!(off.kinds(), [crate::offload::JobKind::Track, crate::offload::JobKind::Warp]);
    let center = tracked_center(&s, clip, tracker);
    s.execute("prop.set", json!({"layer": clip.0, "prop": center, "value": [180.0, 140.0]})).unwrap();
    off.run(s.footage.clone());
    assert!(s.poll_offload());
    let track = s.offload_log.iter().find(|(kind, _)| *kind == crate::offload::JobKind::Track).unwrap();
    assert!(track.1.as_ref().is_some_and(|e| e.contains("nothing was applied")));
    let warp = s.offload_log.iter().find(|(kind, _)| *kind == crate::offload::JobKind::Warp).unwrap();
    assert!(warp.1.is_none(), "the other analysis completed successfully: {warp:?}");
    assert_eq!(s.execute("warp.status", json!({"layer": clip.0, "effect": effect})).unwrap()["analyzed"], true);
    assert!(s.active_comp().unwrap().layer(clip).unwrap().props.find(center).unwrap().keys.is_empty());
}

/// M13.32: where jobs run elsewhere (the browser), `warp.status` never solves the
/// stabilization plan on the page's thread (seconds for a long Subspace Warp clip): the
/// analysis job sends the plan's summary with its result, and a settings change has the plan
/// solved by a job ("stabilizing" meanwhile).
#[test]
fn warp_plan_is_solved_in_the_job_not_on_the_page() {
    let (mut s, clip, solid) = setup(shaky);
    s.execute("edit.clear", json!({"layers": [solid.0]})).unwrap();
    s.execute("layer.select", json!({"layers": [clip.0]})).unwrap();
    s.execute("effect.apply", json!({"effect": "Warp Stabilizer"})).unwrap();
    // Settings no other test uses: the plans of this test are solved nowhere else.
    set_param(&mut s, clip, "stabilization/smoothness", json!(41.25));
    let off = std::sync::Arc::new(Deferred::default());
    s.offload = Some(off.clone());
    s.execute_checked("warp.analyze", json!({"layer": clip.0})).unwrap();
    assert_eq!(off.kinds(), [crate::offload::JobKind::Warp]);
    off.run(s.footage.clone());
    assert!(s.poll_offload());
    let st = s.execute("warp.status", json!({})).unwrap();
    assert_eq!(st["analyzed"], true, "{st}");
    assert!(st.get("stabilizing").is_none() && st["autoScale"].as_f64().is_some_and(|a| a > 100.0), "the plan came with the analysis: {st}");
    assert!(off.kinds().is_empty(), "nothing more to solve");

    // A settings change: the page doesn't solve the new plan, a job does.
    set_param(&mut s, clip, "stabilization/smoothness", json!(23.75));
    let st = s.execute("warp.status", json!({})).unwrap();
    assert_eq!(st["stabilizing"], true, "{st}");
    assert!(st.get("autoScale").is_none() && st["analyzed"] == true, "{st}");
    s.execute("warp.status", json!({})).unwrap();
    assert_eq!(off.kinds(), [crate::offload::JobKind::WarpPlan], "one job, however often the status is asked");
    assert!(s.jobs().iter().any(|j| j.id == "warpPlan"));
    let rev = s.revision;
    off.run(s.footage.clone());
    assert!(s.poll_offload());
    assert_eq!(s.revision, rev, "a plan summary changes no frames");
    let st = s.execute("warp.status", json!({})).unwrap();
    assert!(st.get("stabilizing").is_none(), "{st}");
    let w: [[f64; 3]; 3] = serde_json::from_value(st["warp"].clone()).unwrap_or_else(|_| panic!("{st}"));
    // The same plan as solved here (without jobs).
    s.offload = None;
    let here = s.execute("warp.status", json!({})).unwrap();
    assert_eq!(here["warp"], json!(w));
    assert_eq!(here["autoScale"], st["autoScale"]);
}
